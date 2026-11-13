//! Persistent, single-reader PTY sessions for trusted shell commands.
//!
//! Frames are in-band, not authentication: commands able to inspect or modify the
//! shell can forge them. Output can also contain prompts and background output.

mod frame;
pub mod output;
mod scratch;

use frame::{FrameParser, FramePlatform, FrameStatus};
use output::{OutputCapture, OutputView};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use scratch::Scratch;
use std::io::{self, Read, Write};
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct PtyError {
    pub message: String,
    /// Scratch retained because cleanup failed or a process tree cannot be proven stopped.
    pub retained_scratch: Option<std::path::PathBuf>,
    pub partial_output: Vec<u8>,
    /// True when older partial-output bytes were discarded to enforce the limit.
    pub partial_output_truncated: bool,
}
impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if self.partial_output_truncated { write!(f, "; partial output truncated")?; }
        if let Some(path) = &self.retained_scratch { write!(f, "; scratch retained at {} (process tree status uncertain)", path.display())?; }
        Ok(())
    }
}
impl std::error::Error for PtyError {}

#[derive(Debug)]
pub enum PtyCommandOutcome {
    Completed { exit_code: i32, output: OutputView },
    /// `partial_output_truncated` indicates older bytes were discarded at the partial-output limit.
    ShellExited { exit_status: Option<String>, partial_output: Vec<u8>, partial_output_truncated: bool },
}

struct ReceivedOutput {
    capture: Option<OutputCapture>,
    partial: Vec<u8>,
    limit: usize,
    truncated: bool,
}

impl ReceivedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        let capture_error = self.capture.as_mut().and_then(|capture| capture.write_chunk(bytes).err());
        if capture_error.is_some() {
            self.truncated = true;
        }
        self.truncated |= bytes.len() > self.limit.saturating_sub(self.partial.len());
        if bytes.len() >= self.limit {
            self.partial.clear();
            self.partial.extend_from_slice(&bytes[bytes.len() - self.limit..]);
        } else {
            let overflow = (self.partial.len() + bytes.len()).saturating_sub(self.limit);
            if overflow > 0 {
                self.partial.drain(..overflow);
            }
            self.partial.extend_from_slice(bytes);
        }
        match capture_error {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

enum ReceiveOutcome {
    Completed { exit_code: i32, output: Option<OutputView>, partial: Vec<u8>, partial_output_truncated: bool },
    ShellExited { exit_status: Option<String>, partial_output: Vec<u8>, partial_output_truncated: bool },
}
enum ReadEvent { Data(Vec<u8>), End(io::Result<()>) }

#[cfg(windows)]
mod job {
    use std::{ffi::c_void, io};
    type Handle = *mut c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
    }
    pub struct Job(Handle);
    unsafe impl Send for Job {}
    impl Job {
        pub fn assign(process: std::os::windows::io::RawHandle) -> io::Result<Self> {
            unsafe {
                let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
                if job.is_null() { return Err(io::Error::last_os_error()); }
                if AssignProcessToJobObject(job, process.cast()) == 0 {
                    let error = io::Error::last_os_error(); CloseHandle(job); return Err(error);
                }
                Ok(Self(job))
            }
        }
        pub fn terminate(&self) -> io::Result<()> {
            if unsafe { TerminateJobObject(self.0, 1) } == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
        }
    }
    impl Drop for Job { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
}

#[cfg(target_os = "linux")]
fn child_pid(child: &dyn Child) -> io::Result<i32> {
    child
        .process_id()
        .and_then(|p| i32::try_from(p).ok())
        .filter(|p| *p > 0)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing or invalid child PID"))
}

#[cfg(target_os = "linux")]
fn child_exited(pid: i32) -> io::Result<bool> {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::waitid(libc::P_PID, pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOHANG | libc::WNOWAIT) };
        if rc == 0 {
            return match unsafe { info.si_pid() } {
                0 => Ok(false),
                p if p == pid => Ok(true),
                other => Err(io::Error::new(io::ErrorKind::Other, format!("unexpected waitid PID {other}"))),
            };
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted { return Err(err); }
    }
}

fn stop_processes(master: &dyn MasterPty, child: &mut dyn Child, #[cfg(windows)] job: Option<&job::Job>) -> String {
    let _ = master;
    #[cfg(target_os = "linux")]
    {
        // WNOWAIT keeps the child PID reserved until after group signalling.
        // Only signal the group if the child is known to be its leader.
        let pid = child_pid(child);
        let exited = pid.as_ref().map_or_else(
            |e| Err(io::Error::new(e.kind(), e.to_string())),
            |&pid| child_exited(pid),
        );
        let group_result = match (&pid, &exited) {
            (Ok(pid), Ok(_)) if *pid > 1 => Some({
                let group = unsafe { libc::getpgid(*pid) };
                if group < 0 { Err(io::Error::last_os_error()) }
                else if group != *pid { Err(io::Error::new(io::ErrorKind::Other, "child is not process group leader")) }
                else if unsafe { libc::kill(-*pid, libc::SIGKILL) } == 0 { Ok(()) }
                else { Err(io::Error::last_os_error()) }
            }),
            (Ok(_), Ok(_)) => Some(Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing to signal process group 1"))),
            _ => None,
        };
        let shell_result = if matches!(&exited, Ok(false)) { Some(child.kill()) } else { None };
        format!("child exited: {exited:?}; group termination: {group_result:?}; shell termination: {shell_result:?}; escaped descendants cannot be ruled out")
    }
    #[cfg(windows)]
    {
        let status = child.try_wait();
        let group_result = job.map(|j| j.terminate());
        let shell_result = if matches!(&status, Ok(None)) { Some(child.kill()) } else { None };
        format!("child status: {status:?}; job termination: {group_result:?}; shell termination: {shell_result:?}; escaped descendants cannot be ruled out")
    }
}

/// Observed session state; unavailable or unprovable values are absent.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PtySnapshot {
    pub pwd: Option<String>,
    pub last_exit_code: Option<i32>,
    pub jobs: Option<Vec<String>>,
    pub scratch_names: Option<Vec<String>>,
    pub scratch_total: Option<usize>,
}

pub struct PtyRuntime {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: mpsc::Sender<Vec<u8>>,
    write_results: mpsc::Receiver<io::Result<()>>,
    reader: mpsc::Receiver<ReadEvent>,
    pending: Vec<u8>,
    scratch: Option<Scratch>,
    nonce: String,
    next_id: u64,
    output_view_bytes: usize,
    last_exit_code: Option<i32>,
    platform: FramePlatform,
    #[cfg(windows)]
    job: Option<job::Job>,
    usable: bool,
}

impl PtyRuntime {
    /// Opens a shell and waits for its first frame. Individual OS calls cannot be interrupted.
    pub fn new(startup_timeout: Duration, api_key_env: &str, output_view_bytes: usize) -> Result<Self, PtyError> {
        Self::new_cancellable(startup_timeout, api_key_env, output_view_bytes, &AtomicBool::new(false))
    }

    /// Opens a shell with a cancellable, deadline-bounded startup. Individual OS calls cannot be interrupted.
    pub fn new_cancellable(startup_timeout: Duration, api_key_env: &str, output_view_bytes: usize, cancelled: &AtomicBool) -> Result<Self, PtyError> {
        let start = Instant::now();
        if cancelled.load(Ordering::Acquire) { return Err(error("startup cancelled".into())); }
        if startup_timeout.is_zero() { return Err(error("startup timeout must be nonzero".into())); }
        if output_view_bytes < output::MIN_VIEW_BYTES_FOR_NOTICE {
            return Err(error(format!(
                "output view limit must be at least {} bytes to fit truncation notices",
                output::MIN_VIEW_BYTES_FOR_NOTICE
            )));
        }
        if api_key_env.is_empty() || api_key_env.eq_ignore_ascii_case("FC_SCRATCH") || api_key_env.contains(['=', '\0']) { return Err(error("invalid API key environment variable name".into())); }
        let scratch = Scratch::new().map_err(|e| error(format!("scratch creation failed: {e}")))?;
        macro_rules! check_before_spawn {
            () => {
                if cancelled.load(Ordering::Acquire) || start.elapsed() >= startup_timeout {
                    let message = if cancelled.load(Ordering::Acquire) { "startup cancelled" } else { "startup timed out" };
                    return Err(cleanup_before_spawn(scratch, message.into()));
                }
            };
        }
        check_before_spawn!();
        let mut random = [0u8; 16];
        if let Err(e) = getrandom::getrandom(&mut random) {
            return Err(cleanup_before_spawn(scratch, format!("nonce generation failed: {e}")));
        }
        let nonce: String = random.iter().map(|b| format!("{b:02x}")).collect();
        check_before_spawn!();
        let pair = match native_pty_system().openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }) {
            Ok(pair) => pair,
            Err(e) => return Err(cleanup_before_spawn(scratch, format!("PTY creation failed: {e}"))),
        };
        check_before_spawn!();
        #[cfg(target_os = "linux")]
        let (mut command, platform) = { let mut c = CommandBuilder::new("bash"); c.args(["--noprofile", "--norc", "-i"]); (c, FramePlatform::Linux) };
        #[cfg(windows)]
        let (mut command, platform) = { let mut c = CommandBuilder::new("pwsh.exe"); c.args(["-NoLogo", "-NoProfile"]); (c, FramePlatform::Windows) };
        #[cfg(not(any(target_os = "linux", windows)))]
        compile_error!("ferrocore-runtime-pty supports Linux and Windows only");
        command.env("FC_SCRATCH", scratch.path());
        command.env_remove(api_key_env);
        check_before_spawn!();
        let child = match pair.slave.spawn_command(command) {
            Ok(child) => child,
            Err(e) => return Err(cleanup_before_spawn(scratch, format!("shell spawn failed: {e}"))),
        };
        drop(pair.slave);
        #[cfg(windows)]
        let job = match child.as_raw_handle().ok_or_else(|| io::Error::new(io::ErrorKind::Other, "missing process handle")).and_then(job::Job::assign) {
            Ok(job) => Some(job),
            Err(e) => { let mut child = child; let _ = child.kill(); return Err(PtyError { message: format!("job assignment failed: {e}; process tree status uncertain"), retained_scratch: Some(scratch.path().to_owned()), partial_output: Vec::new(), partial_output_truncated: false }); }
        };
        if cancelled.load(Ordering::Acquire) || start.elapsed() >= startup_timeout {
            let message = if cancelled.load(Ordering::Acquire) { "startup cancelled" } else { "startup timed out" };
            let mut child = child;
            let stop = stop_processes(&*pair.master, &mut *child, #[cfg(windows)] job.as_ref());
            return Err(PtyError { message: format!("{message}; {stop}"), retained_scratch: Some(scratch.path().to_owned()), partial_output: Vec::new(), partial_output_truncated: false });
        }
        let mut reader = match pair.master.try_clone_reader() {
            Ok(reader) => reader,
            Err(e) => { let mut child = child; let stop = stop_processes(&*pair.master, &mut *child, #[cfg(windows)] job.as_ref()); return Err(PtyError { message: format!("PTY reader failed: {e}; {stop}"), retained_scratch: Some(scratch.path().to_owned()), partial_output: Vec::new(), partial_output_truncated: false }); }
        };
        let writer = match pair.master.take_writer() {
            Ok(writer) => writer,
            Err(e) => { let mut child = child; let stop = stop_processes(&*pair.master, &mut *child, #[cfg(windows)] job.as_ref()); return Err(PtyError { message: format!("PTY writer failed: {e}; {stop}"), retained_scratch: Some(scratch.path().to_owned()), partial_output: Vec::new(), partial_output_truncated: false }); }
        };
        let (write_tx, write_rx) = mpsc::channel::<Vec<u8>>();
        let (result_tx, result_rx) = mpsc::channel();
        thread::spawn(move || { let mut writer = writer; while let Ok(bytes) = write_rx.recv() { let result = writer.write_all(&bytes).and_then(|_| writer.flush()); let failed = result.is_err(); if result_tx.send(result).is_err() || failed { break; } } });
        let (tx, rx) = mpsc::sync_channel(16);
        thread::spawn(move || { let mut buf = [0; 8192]; loop { match reader.read(&mut buf) { Ok(0) => { let _ = tx.send(ReadEvent::End(Ok(()))); break; }, Ok(n) => if tx.send(ReadEvent::Data(buf[..n].to_vec())).is_err() { break; }, Err(e) if e.kind() == io::ErrorKind::Interrupted => continue, Err(e) => { let _ = tx.send(ReadEvent::End(Err(e))); break; } } } });
        let mut runtime = Self { master: pair.master, child, writer: write_tx, write_results: result_rx, reader: rx, pending: Vec::new(), scratch: Some(scratch), nonce, next_id: 1, output_view_bytes, last_exit_code: None, platform, usable: true, #[cfg(windows)] job };
        #[cfg(target_os = "linux")]
        let setup = "stty -echo; PS1=''; PS2=''";
        #[cfg(windows)]
        let setup = "Remove-Module PSReadLine -ErrorAction SilentlyContinue; function prompt { '' }";
        let script = runtime.script(setup, 0);
        if cancelled.load(Ordering::Acquire) || start.elapsed() >= startup_timeout {
            let message = if cancelled.load(Ordering::Acquire) { "startup cancelled" } else { "startup timed out" };
            return Err(runtime.fail(message.into(), Vec::new()));
        }
        if let Err(e) = runtime.send(script.into_bytes()) { return Err(runtime.fail(format!("startup write failed: {e}"), Vec::new())); }
        match runtime.receive(0, start, startup_timeout, cancelled, None) {
            Ok(ReceiveOutcome::Completed { exit_code: 0, .. }) => Ok(runtime),
            Ok(ReceiveOutcome::ShellExited { exit_status, partial_output, partial_output_truncated }) => { let mut e = runtime.fail(format!("startup shell exited: {exit_status:?}"), partial_output); e.partial_output_truncated = partial_output_truncated; Err(e) },
            Ok(ReceiveOutcome::Completed { exit_code, partial, partial_output_truncated, .. }) => { let mut e = runtime.fail(format!("startup handshake exit code: {exit_code}"), partial); e.partial_output_truncated = partial_output_truncated; Err(e) },
            Err(e) => Err(e),
        }
    }

    pub fn scratch_path(&self) -> &Path { self.scratch.as_ref().expect("scratch exists").path() }
    pub fn scratch_fallback_used(&self) -> bool { self.scratch.as_ref().expect("scratch exists").fallback_used() }

    /// Reads state without executing a shell command or changing its status.
    /// Jobs and platforms without a reliable cwd query remain unknown.
    pub fn snapshot(&self, scratch_entries: usize) -> PtySnapshot {
        #[cfg(target_os = "linux")]
        let pwd = if self.usable {
            child_pid(&*self.child).ok()
                .and_then(|pid| std::fs::read_link(format!("/proc/{pid}/cwd")).ok())
                .and_then(|path| path.into_os_string().into_string().ok())
        } else { None };
        #[cfg(not(target_os = "linux"))]
        let pwd = None;
        let scratch = std::fs::read_dir(self.scratch_path()).ok().and_then(|entries| {
            let mut names = Vec::new();
            for entry in entries {
                names.push(entry.ok()?.file_name().into_string().ok()?);
            }
            names.sort_unstable();
            let total = names.len();
            names.truncate(scratch_entries);
            Some((names, total))
        });
        let (scratch_names, scratch_total) = match scratch {
            Some((names, total)) => (Some(names), Some(total)),
            None => (None, None),
        };
        PtySnapshot { pwd, last_exit_code: self.last_exit_code, jobs: None, scratch_names, scratch_total }
    }

    /// Stops the session. Scratch is retained when descendant termination cannot be proven.
    pub fn shutdown(mut self) -> Result<(), PtyError> {
        Err(self.fail("session shutdown; process tree status uncertain".into(), Vec::new()))
    }

    /// Commands execute serially in the same shell. Cancellation poisons the session.
    pub fn execute(&mut self, command: &str, timeout: Duration, cancelled: &AtomicBool) -> Result<PtyCommandOutcome, PtyError> {
        if !self.usable { return Err(self.fail("session is closed".into(), Vec::new())); }
        if cancelled.load(Ordering::Acquire) { return Err(self.fail("command cancelled".into(), Vec::new())); }
        if timeout.is_zero() { return Err(self.fail("command timeout must be nonzero".into(), Vec::new())); }
        let start = Instant::now();
        self.last_exit_code = None;
        let id = self.next_id;
        self.next_id = match id.checked_add(1) { Some(id) => id, None => return Err(self.fail("command ID exhausted".into(), Vec::new())) };
        let capture = match OutputCapture::new(self.scratch_path(), self.output_view_bytes) {
            Ok(capture) => capture,
            Err(e) => return Err(self.fail(format!("output capture creation failed: {e}"), Vec::new())),
        };
        let script = self.script(command, id);
        if cancelled.load(Ordering::Acquire) || start.elapsed() >= timeout {
            let message = if cancelled.load(Ordering::Acquire) { "command cancelled" } else { "command timed out" };
            return Err(self.fail(message.into(), Vec::new()));
        }
        if let Err(e) = self.send(script.into_bytes()) { return Err(self.fail(format!("command write failed: {e}"), Vec::new())); }
        match self.receive(id, start, timeout, cancelled, Some(capture))? {
            ReceiveOutcome::Completed { exit_code, output: Some(output), .. } => {
                self.last_exit_code = Some(exit_code);
                Ok(PtyCommandOutcome::Completed { exit_code, output })
            },
            ReceiveOutcome::Completed { output: None, .. } => unreachable!("command capture is present"),
            ReceiveOutcome::ShellExited { exit_status, partial_output, partial_output_truncated } => Ok(PtyCommandOutcome::ShellExited { exit_status, partial_output, partial_output_truncated }),
        }
    }

    fn script(&self, command: &str, id: u64) -> String {
        match self.platform {
            FramePlatform::Linux => format!("{command}\n__fc_ec=$?; printf '\\036__FC_END__:%s:%s:%d__\\037\\n' '{}' '{id}' \"$__fc_ec\"\n", self.nonce),
            FramePlatform::Windows => {
                // Only a single simple resolved command has an attributable status.
                // ID zero is reserved for the trusted startup function definition.
                if id == 0 {
                    return format!(
                        "{command}; $__fc_ec = if ($?) {{ 0 }} else {{ 1 }}; [Console]::WriteLine(('__FC_' + 'END__:' + '{}' + ':{id}:' + $__fc_ec + '__'))\r",
                        self.nonce
                    );
                }
                let literal = command.replace('\'', "''");
                format!("$__fc_native=$false; $__fc_managed=$false; $__fc_tokens=$null; $__fc_errors=$null; $__fc_ast=[System.Management.Automation.Language.Parser]::ParseInput('{literal}', [ref]$__fc_tokens, [ref]$__fc_errors); if ($__fc_errors.Count -eq 0 -and $null -eq $__fc_ast.ParamBlock -and $null -eq $__fc_ast.BeginBlock -and $null -eq $__fc_ast.ProcessBlock -and $null -eq $__fc_ast.DynamicParamBlock -and $__fc_ast.UsingStatements.Count -eq 0 -and $null -ne $__fc_ast.EndBlock -and $__fc_ast.EndBlock.Statements.Count -eq 1) {{ $__fc_stmt=$__fc_ast.EndBlock.Statements[0]; if ($__fc_stmt -is [System.Management.Automation.Language.PipelineAst] -and -not $__fc_stmt.Background -and $__fc_stmt.PipelineElements.Count -eq 1 -and $__fc_stmt.PipelineElements[0] -is [System.Management.Automation.Language.CommandAst]) {{ $__fc_cmd=$__fc_stmt.PipelineElements[0]; $__fc_simple=$__fc_cmd.Redirections.Count -eq 0 -and $__fc_cmd.InvocationOperator -ne [System.Management.Automation.Language.TokenKind]::Dot; foreach ($__fc_element in $__fc_cmd.CommandElements) {{ if ($__fc_element -isnot [System.Management.Automation.Language.StringConstantExpressionAst] -and $__fc_element -isnot [System.Management.Automation.Language.CommandParameterAst]) {{ $__fc_simple=$false }}; if ($__fc_element -is [System.Management.Automation.Language.CommandParameterAst] -and $null -ne $__fc_element.Argument) {{ $__fc_simple=$false }} }}; $__fc_name=$__fc_cmd.GetCommandName(); if ($__fc_simple -and $null -ne $__fc_name -and -not [System.Management.Automation.WildcardPattern]::ContainsWildcardCharacters($__fc_name)) {{ try {{ $__fc_resolved=Get-Command -Name $__fc_name -ErrorAction Stop | Select-Object -First 1; $__fc_native=$__fc_resolved.CommandType -eq [System.Management.Automation.CommandTypes]::Application; $__fc_managed=$__fc_resolved.CommandType -in @([System.Management.Automation.CommandTypes]::Cmdlet, [System.Management.Automation.CommandTypes]::Function) }} catch {{ }} }} }} }}; $global:LASTEXITCODE=$null\r{command}\r$__fc_ok=$?; if ($__fc_native) {{ if ($__fc_ok -and $null -ne $global:LASTEXITCODE -and $global:LASTEXITCODE -eq 0) {{ $__fc_ec=0 }} elseif (-not $__fc_ok -and $null -ne $global:LASTEXITCODE -and $global:LASTEXITCODE -ne 0) {{ $__fc_ec=$global:LASTEXITCODE }} else {{ $__fc_ec='AMBIGUOUS' }} }} elseif ($__fc_managed) {{ if ($__fc_ok) {{ $__fc_ec=0 }} else {{ $__fc_ec=1 }} }} else {{ $__fc_ec='AMBIGUOUS' }}; [Console]::WriteLine(('__FC_' + 'END__:' + '{}' + ':{id}:' + $__fc_ec + '__'))\r", self.nonce)
            },
        }
    }
    fn send(&self, bytes: Vec<u8>) -> io::Result<()> { self.writer.send(bytes).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "PTY writer disconnected")) }
    fn receive(&mut self, id: u64, start: Instant, timeout: Duration, cancelled: &AtomicBool, capture: Option<OutputCapture>) -> Result<ReceiveOutcome, PtyError> {
        let mut parser = FrameParser::new(self.platform, &self.nonce, id);
        let mut output = ReceivedOutput { capture, partial: Vec::new(), limit: self.output_view_bytes.max(output::DEFAULT_VIEW_BYTES), truncated: false };
        macro_rules! record {
            ($bytes:expr) => {
                if let Err(e) = output.write(&$bytes) {
                    let mut failure = self.fail(format!("output capture write failed: {e}"), output.partial);
                    failure.partial_output_truncated = output.truncated;
                    return Err(failure);
                }
            };
        }
        macro_rules! fail_output {
            ($message:expr) => {{
                let mut failure = self.fail($message, output.partial);
                failure.partial_output_truncated = output.truncated;
                failure
            }};
        }
        let mut pending = std::mem::take(&mut self.pending);
        let mut exited: Option<String> = None;
        let mut exit_observed = None;
        loop {
            if exited.is_none() {
                #[cfg(target_os = "linux")]
                match child_pid(&*self.child).and_then(child_exited) {
                    Ok(true) => {
                        let _ = stop_processes(&*self.master, &mut *self.child);
                        self.usable = false;
                        match self.child.try_wait() {
                            Ok(Some(status)) => {
                                exited = Some(status.to_string());
                                exit_observed = Some(Instant::now());
                            }
                            Ok(None) => { record!(parser.finish()); return Err(fail_output!("shell status failed: exited child had no wait status".into())); }
                            Err(e) => { record!(parser.finish()); return Err(fail_output!(format!("shell status failed: {e}"))); }
                        }
                    }
                    Ok(false) => (),
                    Err(e) => { record!(parser.finish()); return Err(fail_output!(format!("shell status failed: {e}"))); }
                }
                #[cfg(windows)]
                match self.child.try_wait() {
                    Ok(Some(status)) => {
                        exited = Some(status.to_string());
                        exit_observed = Some(Instant::now());
                        let _ = stop_processes(&*self.master, &mut *self.child, self.job.as_ref());
                        self.usable = false;
                    }
                    Ok(None) => (),
                    Err(e) => { record!(parser.finish()); return Err(fail_output!(format!("shell status failed: {e}"))); }
                }
            }
            while let Ok(result) = self.write_results.try_recv() {
                if let Err(e) = result { record!(parser.finish()); return Err(fail_output!(format!("PTY write failed: {e}"))); }
            }
            let exit_drain_expired = self.platform == FramePlatform::Windows
                && exit_observed.is_some_and(|at| at.elapsed() >= Duration::from_millis(250));
            if cancelled.load(Ordering::Acquire) || start.elapsed() >= timeout || exit_drain_expired {
                let message = if cancelled.load(Ordering::Acquire) { "command cancelled" } else if exited.is_some() { "shell exited; timed out waiting for PTY EOF" } else { "command timed out" };
                let mut failure = self.fail(message.into(), Vec::new());
                output.capture.take();
                if !pending.is_empty() { let events = parser.push(&pending); record!(events.output); record!(events.remainder); pending.clear(); }
                // Termination may release bytes that were still in the PTY. Bound
                // this drain because descendants can retain the slave indefinitely.
                let drain_start = Instant::now();
                let mut drained_to_end = false;
                loop {
                    let remaining = Duration::from_millis(250).saturating_sub(drain_start.elapsed());
                    if remaining.is_zero() { break; }
                    match self.reader.recv_timeout(remaining) {
                        Ok(ReadEvent::Data(bytes)) => { let events = parser.push(&bytes); record!(events.output); record!(events.remainder); },
                        Ok(ReadEvent::End(Ok(()))) => { drained_to_end = true; break; },
                        Ok(ReadEvent::End(Err(_))) | Err(_) => break,
                    }
                }
                if !drained_to_end { failure.message.push_str("; output may be incomplete: PTY drain did not reach EOF"); }
                record!(parser.finish());
                failure.partial_output = output.partial;
                failure.partial_output_truncated = output.truncated;
                return Err(failure);
            }
            let mut remaining = timeout.saturating_sub(start.elapsed()).min(Duration::from_millis(25));
            if self.platform == FramePlatform::Windows {
                if let Some(at) = exit_observed { remaining = remaining.min(Duration::from_millis(250).saturating_sub(at.elapsed())); }
            }
            let event = if pending.is_empty() { self.reader.recv_timeout(remaining) } else { Ok(ReadEvent::Data(std::mem::take(&mut pending))) };
            match event {
                Ok(ReadEvent::Data(bytes)) => {
                    let events = parser.push(&bytes);
                    record!(events.output);
                    for _ in 0..events.cursor_queries { if let Err(e) = self.send(b"\x1b[1;1R".to_vec()) { record!(parser.finish()); return Err(fail_output!(format!("cursor response failed: {e}"))); } }
                    if let Some(status) = events.status {
                        self.pending = events.remainder;
                        record!(parser.finish());
                        return match status {
                            FrameStatus::ExitCode(exit_code) => {
                                let view = match output.capture.take() {
                                    Some(capture) => match capture.finish() {
                                        Ok(view) => Some(view),
                                        Err(e) => return Err(fail_output!(format!("output capture finish failed: {e}"))),
                                    },
                                    None => None,
                                };
                                Ok(ReceiveOutcome::Completed { exit_code, output: view, partial: output.partial, partial_output_truncated: output.truncated })
                            }
                            FrameStatus::Ambiguous => Err(fail_output!("PowerShell command exit status is ambiguous".into())),
                        };
                    }
                }
                Ok(ReadEvent::End(result)) => {
                    record!(parser.finish());
                    let mut status = exited;
                    if let Err(e) = &result {
                        #[cfg(target_os = "linux")]
                        if e.raw_os_error() == Some(libc::EIO) {
                            // The PTY can close just before wait reports the shell exit.
                            let deadline = Instant::now() + Duration::from_millis(100);
                            let mut observed_exit = status.is_some();
                            while !observed_exit {
                                match child_pid(&*self.child).and_then(child_exited) {
                                    Ok(true) => observed_exit = true,
                                    Ok(false) if Instant::now() < deadline => thread::sleep(Duration::from_millis(5)),
                                    _ => break,
                                }
                            }
                            if !observed_exit { return Err(fail_output!(format!("PTY read failed: {e}"))); }
                        } else {
                            return Err(fail_output!(format!("PTY read failed: {e}")));
                        }
                        #[cfg(not(target_os = "linux"))]
                        return Err(fail_output!(format!("PTY read failed: {e}")));
                    }
                    #[cfg(windows)]
                    if status.is_none() { status = self.child.try_wait().ok().flatten().map(|s| s.to_string()); }
                    if self.usable {
                        let _ = stop_processes(&*self.master, &mut *self.child, #[cfg(windows)] self.job.as_ref());
                        self.usable = false;
                    }
                    if status.is_none() { status = self.child.try_wait().ok().flatten().map(|s| s.to_string()); }
                    #[cfg(target_os = "linux")]
                    if let Err(e) = &result {
                        if status.is_none() { return Err(fail_output!(format!("PTY read failed: {e}"))); }
                    }
                    return Ok(ReceiveOutcome::ShellExited { exit_status: status, partial_output: output.partial, partial_output_truncated: output.truncated });
                }
                Err(mpsc::RecvTimeoutError::Timeout) => (),
                Err(mpsc::RecvTimeoutError::Disconnected) => { record!(parser.finish()); return Err(fail_output!("PTY reader disconnected".into())); }
            }
        }
    }
    fn fail(&mut self, message: String, partial_output: Vec<u8>) -> PtyError {
        let stop = if self.usable { stop_processes(&*self.master, &mut *self.child, #[cfg(windows)] self.job.as_ref()) } else { "session already closed; process tree status uncertain".into() };
        self.usable = false;
        PtyError { message: format!("{message}; {stop}"), retained_scratch: self.scratch.as_ref().map(|s| s.path().to_owned()), partial_output, partial_output_truncated: false }
    }
}
impl Drop for PtyRuntime { fn drop(&mut self) { if self.usable { let _ = stop_processes(&*self.master, &mut *self.child, #[cfg(windows)] self.job.as_ref()); } } }
fn error(message: String) -> PtyError { PtyError { message, retained_scratch: None, partial_output: Vec::new(), partial_output_truncated: false } }
fn cleanup_before_spawn(scratch: Scratch, message: String) -> PtyError {
    let path = scratch.path().to_owned();
    match scratch.cleanup() {
        Ok(()) => error(format!("{message}; cleanup: Ok(())")),
        Err(e) => PtyError { message: format!("{message}; cleanup failed: {e}"), retained_scratch: Some(path), partial_output: Vec::new(), partial_output_truncated: false },
    }
}

#[cfg(test)]
mod partial_output_tests {
    use super::ReceivedOutput;

    #[test]
    fn truncation_tracks_discarded_bytes_across_chunks() {
        let mut output = ReceivedOutput { capture: None, partial: Vec::new(), limit: 4, truncated: false };
        output.write(b"ab").unwrap();
        output.write(b"cd").unwrap();
        assert_eq!(output.partial, b"abcd");
        assert!(!output.truncated);
        output.write(b"e").unwrap();
        assert_eq!(output.partial, b"bcde");
        assert!(output.truncated);

        let mut output = ReceivedOutput { capture: None, partial: Vec::new(), limit: 4, truncated: false };
        output.write(b"abcd").unwrap();
        assert!(!output.truncated);
        output.write(b"efgh").unwrap();
        assert_eq!(output.partial, b"efgh");
        assert!(output.truncated);

        let mut output = ReceivedOutput { capture: None, partial: Vec::new(), limit: 4, truncated: false };
        output.write(b"abcde").unwrap();
        assert_eq!(output.partial, b"bcde");
        assert!(output.truncated);
    }

    #[test]
    fn write_chunk_error_marks_partial_output_truncated_and_undersized_view_fails_early() {
        let dir = std::env::temp_dir().join(format!("ferrocore-partial-err-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let capture = super::OutputCapture::new(&dir, super::output::MIN_VIEW_BYTES_FOR_NOTICE).unwrap();
        let mut output = ReceivedOutput { capture: Some(capture), partial: Vec::new(), limit: 16, truncated: false };
        output.write(b"ok").unwrap();
        assert!(!output.truncated);
        output.capture.as_mut().unwrap().poison_for_test();
        assert!(output.write(b"lost").is_err());
        assert!(output.truncated);
        assert_eq!(output.partial, b"oklost");
        drop(output);
        let _ = std::fs::remove_dir_all(&dir);

        let err = super::PtyRuntime::new(
            std::time::Duration::from_secs(5),
            "FERROCORE_TEST_API_KEY",
            super::output::MIN_VIEW_BYTES_FOR_NOTICE - 1,
        )
        .err()
        .expect("undersized output_view_bytes must fail before spawning");
        assert!(err.retained_scratch.is_none());
        assert!(err.message.contains("at least"), "{err}");
    }
}
