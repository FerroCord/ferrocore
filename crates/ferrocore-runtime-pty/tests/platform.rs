#![cfg(any(target_os = "linux", windows))]

use ferrocore_runtime_pty::{PtyCommandOutcome, PtyRuntime};
use std::{sync::atomic::AtomicBool, time::Duration};

const STARTUP: Duration = Duration::from_secs(10);
const COMMAND: Duration = Duration::from_secs(10);


// Each Windows test gets its own inherited TEMP, avoiding process-wide environment races.
#[cfg(windows)]
fn isolate_windows_test(insecure: bool) -> bool {
    use std::{ffi::c_void, ptr, sync::atomic::{AtomicU64, Ordering}};
    if std::env::var_os("FERROCORE_PTY_TEST_CHILD").is_some() { return false; }
    type Handle = *mut c_void;
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(token: Handle, class: u32, info: *mut c_void, length: u32, needed: *mut u32) -> i32;
        fn ConvertSidToStringSidW(sid: Handle, text: *mut *mut u16) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: Handle) -> Handle;
    }
    let user = unsafe {
        let mut token = ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), 8, &mut token), 0);
        let mut needed = 0;
        GetTokenInformation(token, 1, ptr::null_mut(), 0, &mut needed);
        assert!(needed > 0);
        let mut data = vec![0usize; (needed as usize + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()];
        assert_ne!(GetTokenInformation(token, 1, data.as_mut_ptr().cast(), needed, &mut needed), 0);
        let mut text = ptr::null_mut();
        assert_ne!(ConvertSidToStringSidW(*(data.as_ptr() as *const Handle), &mut text), 0);
        let mut len = 0;
        while *text.add(len) != 0 { len += 1; }
        let sid = String::from_utf16(std::slice::from_raw_parts(text, len)).unwrap();
        LocalFree(text.cast());
        CloseHandle(token);
        sid
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = loop {
        let path = std::env::temp_dir().join(format!("ferrocore-pty-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        match std::fs::create_dir(&path) {
            Ok(()) => break path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("cannot create isolated TEMP: {e}"),
        }
    };
    let status = std::process::Command::new("icacls").arg(&dir)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("*{user}:(OI)(CI)F"))
        .args(["*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"])
        .status().expect("icacls required for isolated TEMP");
    assert!(status.success(), "could not secure isolated TEMP");
    if insecure {
        let status = std::process::Command::new("icacls").arg(&dir)
            .args(["/grant", "*S-1-1-0:(OI)(CI)F"]).status().unwrap();
        assert!(status.success(), "could not create insecure TEMP fixture");
    }
    let test_name = std::thread::current().name().expect("test thread name").to_owned();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test_name, "--nocapture"])
        .env("FERROCORE_PTY_TEST_CHILD", "1").env("TEMP", &dir).env("TMP", &dir)
        .status().expect("cannot spawn isolated test");
    assert!(status.success(), "isolated test {test_name} failed: {status}");
    // Scratch may be retained because descendant termination is not proven.
    true
}

#[cfg(not(windows))]
fn isolate_windows_test(_: bool) -> bool { false }
fn session() -> PtyRuntime {
    // A missing shell, PTY, or secure scratch directory is a test failure.
    PtyRuntime::new(STARTUP, "FERROCORE_TEST_API_KEY", 16_384).expect("platform PTY prerequisites and startup handshake")
}

fn completed(pty: &mut PtyRuntime, command: &str) -> (i32, String) {
    match pty.execute(command, COMMAND, &AtomicBool::new(false)).expect("command execution") {
        PtyCommandOutcome::Completed { exit_code, output } => (exit_code, output.text),
        other => panic!("expected completed command, got {other:?}"),
    }
}

#[test]
fn pre_cancelled_startup_does_not_spawn() {
    let result = PtyRuntime::new_cancellable(STARTUP, "FERROCORE_TEST_API_KEY", 16_384, &AtomicBool::new(true));
    let error = result.err().expect("cancelled startup must fail");
    assert!(error.message.contains("cancelled"));
    assert!(error.retained_scratch.is_none());
}

#[test]
fn snapshot_reports_bounded_scratch_and_last_exit() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let initial = pty.snapshot(1);
    assert_eq!(initial.last_exit_code, None);
    assert_eq!(initial.jobs, None);
    std::fs::write(pty.scratch_path().join("alpha"), b"a").unwrap();
    std::fs::write(pty.scratch_path().join("beta"), b"b").unwrap();
    let snapshot = pty.snapshot(1);
    assert_eq!(snapshot.scratch_total, Some(2));
    assert_eq!(snapshot.scratch_names, Some(vec!["alpha".into()]));
    #[cfg(target_os = "linux")]
    let command = "sh -c 'exit 23'";
    #[cfg(windows)]
    let command = "& pwsh.exe -NoProfile -Command 'exit 23'";
    assert_eq!(completed(&mut pty, command).0, 23);
    assert_eq!(pty.snapshot(1).last_exit_code, Some(23));
    assert_eq!(pty.snapshot(1).last_exit_code, Some(23));
}

#[cfg(target_os = "linux")]
#[test]
fn snapshot_reads_shell_cwd_without_changing_it() {
    let mut pty = session();
    assert_eq!(completed(&mut pty, "cd /tmp").0, 0);
    assert_eq!(pty.snapshot(10).pwd.as_deref(), Some("/tmp"));
    assert!(completed(&mut pty, "printf '%s' \"$PWD\"").1.contains("/tmp"));
}

#[test]
fn snapshot_does_not_claim_scratch_after_directory_disappears() {
    if isolate_windows_test(false) { return; }
    let pty = session();
    std::fs::remove_dir(pty.scratch_path()).unwrap();
    let snapshot = pty.snapshot(10);
    assert_eq!(snapshot.scratch_names, None);
    assert_eq!(snapshot.scratch_total, None);
}

#[cfg(target_os = "linux")]
#[test]
fn bash_persists_working_directory_and_reports_native_failure() {
    let mut pty = session();
    assert_eq!(completed(&mut pty, "cd /tmp").0, 0);
    let (code, output) = completed(&mut pty, "printf 'FC_PWD=%s\\n' \"$PWD\"");
    assert_eq!(code, 0);
    assert!(output.contains("FC_PWD=/tmp"), "{output:?}");
    assert_eq!(completed(&mut pty, "sh -c 'exit 23'").0, 23);
    assert_eq!(completed(&mut pty, "true").0, 0);
}

#[cfg(windows)]
#[test]
fn powershell_persists_directory_and_reports_native_and_cmdlet_status() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let temp = std::env::temp_dir();
    let set_location = format!("Set-Location -LiteralPath '{}'", temp.to_string_lossy().replace('\'', "''"));
    assert_eq!(completed(&mut pty, &set_location).0, 0);
    let (code, output) = completed(&mut pty, "& pwsh.exe -NoProfile -Command 'if ((Get-Location).Path -eq $env:TEMP) { Write-Output FC_LOCATION_OK }'");
    assert_eq!(code, 0);
    assert!(output.contains("FC_LOCATION_OK"), "{output:?}");
    assert_eq!(completed(&mut pty, "& pwsh.exe -NoProfile -Command 'exit 0'").0, 0);
    assert_eq!(completed(&mut pty, "& pwsh.exe -NoProfile -Command 'exit 23'").0, 23);
    assert_eq!(completed(&mut pty, "& pwsh.exe -NoProfile -Command 'exit -2147483648'").0, i32::MIN);
    assert_eq!(completed(&mut pty, "Get-Item -LiteralPath 'Z:\\__ferrocore_missing__' -ErrorAction SilentlyContinue").0, 1);
    assert_eq!(completed(&mut pty, "Write-Output 'FC_CMDLET_OK'").0, 0);
}

#[cfg(windows)]
#[test]
fn powershell_multi_statement_failure_with_unprovable_status_is_ambiguous() {
    if isolate_windows_test(false) { return; }
    for command in [
        "& pwsh.exe -NoProfile -Command 'exit 23'; Write-Output 'FC_RECOVERED'",
        "Write-Output 'FC_FIRST'; Write-Output 'FC_SECOND'",
        "& pwsh.exe -NoProfile -Command 'exit 0' | Out-Null",
        "Write-Output $(& pwsh.exe -NoProfile -Command 'exit 23')",
        "& pwsh.exe -NoProfile -Command 'exit 0'; Get-Item -LiteralPath 'Z:\\__ferrocore_missing__' -ErrorAction SilentlyContinue",
        "Get-Item -LiteralPath 'Z:\\__ferrocore_missing__' -ErrorAction SilentlyContinue; & pwsh.exe -NoProfile -Command 'exit 19'",
    ] {
        let mut pty = session();
        let error = pty.execute(command, COMMAND, &AtomicBool::new(false)).expect_err("unprovable status must be ambiguous");
        assert!(error.message.contains("ambiguous"), "unexpected error for {command:?}: {error}");
        assert!(pty.execute("Write-Output 'FC_UNEXPECTED'", COMMAND, &AtomicBool::new(false)).is_err());
    }
}
#[cfg(windows)]
#[test]
fn powershell_conditional_native_failure_is_not_reported_as_success() {
    if isolate_windows_test(false) { return; }
    for command in [
        "if ($true) { & pwsh.exe -NoProfile -Command 'exit 19' }",
        "if ($true) { & pwsh.exe -NoProfile -Command 'exit 0' }",
        "if ($false) { Write-Output 'FC_SKIPPED' }",
        "if ($true) { & pwsh.exe -NoProfile -Command 'exit 19'; Write-Output 'FC_RECOVERED' }",
    ] {
        let mut pty = session();
        let error = pty.execute(command, COMMAND, &AtomicBool::new(false)).expect_err("conditional status must be ambiguous");
        assert!(error.message.contains("ambiguous"), "unexpected error for {command:?}: {error}");
        assert!(pty.execute("Write-Output 'FC_UNEXPECTED'", COMMAND, &AtomicBool::new(false)).is_err());
    }
}


#[test]
fn shell_exit_reports_partial_output_and_closes_session() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    #[cfg(target_os = "linux")]
    let command = "printf 'FC_BEFORE_EXIT\\n'; exit 7";
    #[cfg(windows)]
    let command = "[Console]::WriteLine('FC_BEFORE_EXIT'); exit 7";
    match pty.execute(command, COMMAND, &AtomicBool::new(false)) {
        Ok(PtyCommandOutcome::ShellExited { partial_output, partial_output_truncated, .. }) => {
            assert!(!partial_output_truncated);
            assert!(String::from_utf8_lossy(&partial_output).contains("FC_BEFORE_EXIT"));
        }
        #[cfg(windows)]
        Err(error) => {
            assert!(!error.partial_output_truncated);
            assert!(error.message.contains("EOF"), "unexpected error: {error}");
            assert!(error.message.contains("output may be incomplete"), "unexpected error: {error}");
            assert!(String::from_utf8_lossy(&error.partial_output).contains("FC_BEFORE_EXIT"));
        }
        other => panic!("expected shell exit or Windows EOF error, got {other:?}"),
    }
    assert!(pty.execute("echo unexpected", COMMAND, &AtomicBool::new(false)).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn linux_pty_closure_preserves_exit_output() {
    let mut pty = session();
    match pty.execute("printf FC_PTY_EIO; exit 7", COMMAND, &AtomicBool::new(false)) {
        Ok(PtyCommandOutcome::ShellExited { exit_status, partial_output, partial_output_truncated }) => {
            assert!(!partial_output_truncated);
            assert!(exit_status.is_some());
            assert!(String::from_utf8_lossy(&partial_output).contains("FC_PTY_EIO"));
        }
        other => panic!("expected shell exit after PTY closure, got {other:?}"),
    }
}

#[test]
fn shell_exit_reports_partial_output_truncation() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    #[cfg(target_os = "linux")]
    let command = "printf 'FC_HEAD_DROP\\n'; printf '%020000d' 0; printf '\\nFC_TAIL_KEEP\\n'; exit 7";
    #[cfg(windows)]
    let command = "[Console]::WriteLine(('FC_HEAD_' + 'DROP')); [Console]::WriteLine(('X' * 20480)); [Console]::WriteLine(('FC_TAIL_' + 'KEEP')); exit 7";
    match pty.execute(command, COMMAND, &AtomicBool::new(false)) {
        Ok(PtyCommandOutcome::ShellExited { partial_output, partial_output_truncated, .. }) => {
            assert!(partial_output_truncated);
            assert_eq!(partial_output.len(), 16_384);
            let text = String::from_utf8_lossy(&partial_output);
            assert!(text.contains("FC_TAIL_KEEP"), "missing tail: {text:?}");
            assert!(!text.contains("FC_HEAD_DROP"), "head should be truncated: {text:?}");
        }
        #[cfg(windows)]
        Err(error) => {
            assert!(error.partial_output_truncated);
            assert!(error.to_string().contains("partial output truncated"));
            assert_eq!(error.partial_output.len(), 16_384);
            let text = String::from_utf8_lossy(&error.partial_output);
            assert!(text.contains("FC_TAIL_KEEP"), "missing tail: {text:?}");
            assert!(!text.contains("FC_HEAD_DROP"), "head should be truncated: {text:?}");
        }
        other => panic!("expected shell exit or Windows EOF error, got {other:?}"),
    }
}

#[test]
fn timeout_reports_partial_output_truncation() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    #[cfg(windows)]
    assert_eq!(completed(&mut pty, "Write-Output 'FC_WARMUP'").0, 0);
    #[cfg(target_os = "linux")]
    let (command, timeout) = ("printf 'FC_HEAD_DROP\\n'; printf '%020000d' 0; printf '\\nFC_TAIL_KEEP\\n'; sleep 30", Duration::from_secs(2));
    #[cfg(windows)]
    let (command, timeout) = ("[Console]::WriteLine(('FC_HEAD_' + 'DROP')); [Console]::WriteLine(('X' * 20480)); [Console]::WriteLine(('FC_TAIL_' + 'KEEP')); Start-Sleep -Seconds 60", Duration::from_secs(6));
    let error = pty.execute(command, timeout, &AtomicBool::new(false)).expect_err("command must time out");
    assert!(error.partial_output_truncated);
    assert!(error.to_string().contains("partial output truncated"));
    assert_eq!(error.partial_output.len(), 16_384);
    let text = String::from_utf8_lossy(&error.partial_output);
    assert!(text.contains("FC_TAIL_KEEP"), "missing tail: {text:?}");
    assert!(!text.contains("FC_HEAD_DROP"), "head should be truncated: {text:?}");
}

#[cfg(windows)]
#[test]
fn conpty_split_cursor_query_does_not_leak_into_output() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let (code, output) = completed(&mut pty, "& pwsh.exe -NoProfile -Command \"[Console]::Write([char]27); Start-Sleep -Milliseconds 100; [Console]::Write('[6n'); [Console]::WriteLine('FC_AFTER_QUERY')\"");
    assert_eq!(code, 0);
    assert!(output.contains("FC_AFTER_QUERY"), "{output:?}");
    assert!(!output.contains("\x1b[6n"), "{output:?}");
    assert_eq!(completed(&mut pty, "Write-Output 'FC_NEXT_COMMAND'").0, 0);
}

#[cfg(target_os = "linux")]
#[test]
fn shell_exit_drains_delayed_descendant_output() {
    let mut pty = session();
    let scratch = pty.scratch_path().to_owned();
    let command = "(sleep 0.05; printf 'FC_DELAYED_OUTPUT\\n') & exit 7";
    match pty.execute(command, COMMAND, &AtomicBool::new(false)).expect("shell exit") {
        PtyCommandOutcome::ShellExited { partial_output, .. } => {
            assert!(String::from_utf8_lossy(&partial_output).contains("FC_DELAYED_OUTPUT"));
        }
        other => panic!("expected shell exit with delayed output, got {other:?}"),
    }
    assert!(scratch.is_dir());
    assert!(pty.execute("echo unexpected", COMMAND, &AtomicBool::new(false)).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn shell_exit_with_background_process_retains_scratch_and_rejects_commands() {
    let mut pty = session();
    let scratch = pty.scratch_path().to_owned();
    let command = "(sleep 1; printf survived > \"$FC_SCRATCH/background-survived\") & exit 7";
    assert!(matches!(pty.execute(command, COMMAND, &AtomicBool::new(false)).expect("shell exit"), PtyCommandOutcome::ShellExited { .. }));
    assert!(scratch.is_dir());
    let error = pty.execute("echo unexpected", COMMAND, &AtomicBool::new(false)).expect_err("closed session must reject commands");
    assert!(error.message.contains("uncertain"), "unexpected error: {error}");
    assert_eq!(error.retained_scratch.as_deref(), Some(scratch.as_path()));
    drop(pty);
    assert!(scratch.is_dir());
}

#[test]
fn timeout_keeps_output_received_near_deadline() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let scratch = pty.scratch_path().to_owned();
    #[cfg(windows)]
    assert_eq!(completed(&mut pty, "Write-Output 'FC_WARMUP'").0, 0);
    #[cfg(target_os = "linux")]
    let (command, timeout) = ("sleep 0.5; printf 'FC_LAST_BEFORE_TIMEOUT\\n'; sleep 5", Duration::from_secs(2));
    // Warm up the pre-flight parser path first and give ConPTY/PowerShell enough
    // headroom to emit the marker before the deadline under parallel `cargo test`.
    #[cfg(windows)]
    let (command, timeout) = ("Start-Sleep -Seconds 2; [Console]::WriteLine(('FC_LAST_' + 'BEFORE_TIMEOUT')); Start-Sleep -Seconds 60", Duration::from_secs(6));
    let error = pty.execute(command, timeout, &AtomicBool::new(false)).expect_err("command must time out");
    assert!(error.message.contains("timed out"), "{error}");
    assert!(String::from_utf8_lossy(&error.partial_output).contains("FC_LAST_BEFORE_TIMEOUT"), "partial output: {:?}", String::from_utf8_lossy(&error.partial_output));
    assert_eq!(error.retained_scratch.as_deref(), Some(scratch.as_path()));
    assert!(scratch.is_dir());
    assert!(pty.execute("echo unexpected", COMMAND, &AtomicBool::new(false)).is_err());
}

#[test]
fn timeout_retains_scratch_and_closes_session() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let scratch = pty.scratch_path().to_owned();
    #[cfg(windows)]
    assert_eq!(completed(&mut pty, "Write-Output 'FC_WARMUP'").0, 0);
    #[cfg(target_os = "linux")]
    let (command, timeout) = ("printf 'FC_WAITING\\n'; sleep 5", Duration::from_secs(1));
    #[cfg(windows)]
    let (command, timeout) = ("[Console]::WriteLine(('FC_' + 'WAITING')); Start-Sleep -Seconds 60", Duration::from_secs(6));
    let error = pty.execute(command, timeout, &AtomicBool::new(false)).expect_err("command must time out");
    assert!(error.message.contains("timed out"), "{error}");
    assert!(String::from_utf8_lossy(&error.partial_output).contains("FC_WAITING"), "partial output: {:?}", String::from_utf8_lossy(&error.partial_output));
    assert_eq!(error.retained_scratch.as_deref(), Some(scratch.as_path()));
    assert!(scratch.is_dir());
    assert!(pty.execute("echo unexpected", COMMAND, &AtomicBool::new(false)).is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn scratch_is_private_and_exposed_to_shell() {
    use std::os::unix::fs::PermissionsExt;
    let mut pty = session();
    let path = pty.scratch_path().to_owned();
    let metadata = std::fs::symlink_metadata(&path).unwrap();
    assert!(metadata.file_type().is_dir());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    assert!(path.starts_with(if pty.scratch_fallback_used() { "/tmp" } else { "/dev/shm" }));
    let (code, output) = completed(&mut pty, "printf 'FC_SCRATCH_VALUE=%s\\n' \"$FC_SCRATCH\"");
    assert_eq!(code, 0);
    assert!(output.contains(&format!("FC_SCRATCH_VALUE={}", path.display())), "{output:?}");
}

#[cfg(windows)]
#[test]
fn windows_scratch_is_exposed_and_terminal_is_80_by_24() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let path = pty.scratch_path().to_owned();
    assert!(path.is_dir());
    let (code, output) = completed(&mut pty, "& pwsh.exe -NoProfile -Command 'if ($env:FC_SCRATCH -eq (Get-Item -LiteralPath $env:FC_SCRATCH).FullName) { Write-Output FC_SCRATCH_OK }; Write-Output FC_SIZE=$([Console]::WindowWidth)x$([Console]::WindowHeight)'");
    assert_eq!(code, 0);
    assert!(output.contains("FC_SCRATCH_OK"), "{output:?}");
    assert!(output.contains("FC_SIZE=80x24"), "{output:?}");
}

#[cfg(target_os = "linux")]
#[test]
fn terminal_has_expected_dimensions() {
    let mut pty = session();
    let (code, output) = completed(&mut pty, "stty size");
    assert_eq!(code, 0);
    assert!(output.lines().any(|line| line.trim() == "24 80"), "{output:?}");
}

#[test]
fn cancellation_closes_session_and_retains_scratch() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let path = pty.scratch_path().to_owned();
    let error = pty.execute("echo must_not_run", COMMAND, &AtomicBool::new(true)).expect_err("cancelled command must fail");
    assert!(error.message.contains("cancel"), "{error}");
    assert_eq!(error.retained_scratch.as_deref(), Some(path.as_path()));
    assert!(path.is_dir());
    assert!(pty.execute("echo unexpected", COMMAND, &AtomicBool::new(false)).is_err());
}

// Run potentially stuck writes in a subprocess so a regression cannot hang the suite.
fn blocked_write_case(cancel: bool) {
    use std::{sync::{atomic::Ordering, Arc}, thread, time::Instant};
    if isolate_windows_test(false) { return; }
    const CHILD: &str = "FERROCORE_PTY_BLOCKED_WRITE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let test_name = thread::current().name().expect("test thread name").to_owned();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name, "--nocapture"])
            .env(CHILD, "1").spawn().expect("spawn blocked-write test");
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = child.try_wait().expect("wait for blocked-write test") {
                assert!(status.success(), "blocked-write test failed: {status}");
                return;
            }
            if Instant::now() >= deadline {
                child.kill().expect("kill stuck blocked-write test");
                let _ = child.wait();
                panic!("blocked write did not respect cancellation or timeout");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
    let mut pty = session();
    let scratch = pty.scratch_path().to_owned();
    #[cfg(target_os = "linux")]
    let prefix = "sleep 30\n#";
    #[cfg(windows)]
    let prefix = "Start-Sleep -Seconds 30\r#";
    let command = format!("{prefix}{}", "x".repeat(8 * 1024 * 1024));
    let cancelled = Arc::new(AtomicBool::new(false));
    let canceller = if cancel {
        let flag = Arc::clone(&cancelled);
        Some(thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::Release);
        }))
    } else { None };
    let write_timeout = if cancel { Duration::from_secs(5) } else { Duration::from_millis(500) };
    let error = pty.execute(&command, write_timeout, &cancelled).expect_err("blocked write must fail");
    if let Some(canceller) = canceller { canceller.join().unwrap(); }
    assert!(error.message.contains(if cancel { "cancel" } else { "timed out" }), "{error}");
    assert_eq!(error.retained_scratch.as_deref(), Some(scratch.as_path()));
    assert!(scratch.is_dir());
    assert!(pty.execute("echo unexpected", COMMAND, &AtomicBool::new(false)).is_err());
}

#[test]
fn blocked_write_observes_timeout() { blocked_write_case(false); }

#[test]
fn blocked_write_observes_cancellation() { blocked_write_case(true); }

#[cfg(windows)]
#[test]
fn windows_scratch_dacl_has_no_everyone_access() {
    if isolate_windows_test(false) { return; }
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    type Handle = *mut c_void;
    #[repr(C)]
    struct AclInfo { count: u32, used: u32, free: u32 }
    #[repr(C)]
    struct AceHeader { kind: u8, flags: u8, size: u16 }
    #[repr(C)]
    struct AllowedAce { header: AceHeader, mask: u32, sid: u32 }
    #[link(name = "advapi32")]
    extern "system" {
        fn GetNamedSecurityInfoW(name: *const u16, object_type: u32, info: u32, owner: *mut Handle, group: *mut Handle, dacl: *mut Handle, sacl: *mut Handle, descriptor: *mut Handle) -> u32;
        fn GetAclInformation(acl: Handle, info: *mut c_void, length: u32, class: u32) -> i32;
        fn GetAce(acl: Handle, index: u32, ace: *mut Handle) -> i32;
        fn ConvertStringSidToSidW(text: *const u16, sid: *mut Handle) -> i32;
        fn EqualSid(first: Handle, second: Handle) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" { fn LocalFree(memory: Handle) -> Handle; }
    let pty = session();
    unsafe {
        let name: Vec<u16> = pty.scratch_path().as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        assert_eq!(GetNamedSecurityInfoW(name.as_ptr(), 1, 4, ptr::null_mut(), ptr::null_mut(), &mut dacl, ptr::null_mut(), &mut descriptor), 0);
        assert!(!dacl.is_null(), "null DACL grants unrestricted access");
        let mut everyone = ptr::null_mut();
        let sid: Vec<u16> = "S-1-1-0\0".encode_utf16().collect();
        assert_ne!(ConvertStringSidToSidW(sid.as_ptr(), &mut everyone), 0);
        let mut info = AclInfo { count: 0, used: 0, free: 0 };
        assert_ne!(GetAclInformation(dacl, (&mut info as *mut AclInfo).cast(), std::mem::size_of::<AclInfo>() as u32, 2), 0);
        assert!(info.count > 0);
        for index in 0..info.count {
            let mut ace = ptr::null_mut();
            assert_ne!(GetAce(dacl, index, &mut ace), 0);
            if (*(ace as *const AceHeader)).kind == 0 {
                let allowed = &*(ace as *const AllowedAce);
                assert_eq!(EqualSid((&allowed.sid as *const u32).cast_mut().cast(), everyone), 0, "scratch grants Everyone access");
            }
        }
        LocalFree(everyone);
        LocalFree(descriptor);
    }
}

#[cfg(windows)]
#[test]
fn insecure_temp_is_rejected() {
    if isolate_windows_test(true) { return; }
    let error = match PtyRuntime::new(STARTUP, "FERROCORE_TEST_API_KEY", 16_384) {
        Ok(_) => panic!("scratch inherited Everyone access and must be rejected"),
        Err(error) => error,
    };
    assert!(error.message.contains("scratch"), "{error}");
    assert_eq!(std::fs::read_dir(std::env::temp_dir()).unwrap().count(), 0, "rejected scratch must be removed");
}

#[test]
fn completed_command_publishes_last_output_log_and_failure_preserves_prior_log() {
    if isolate_windows_test(false) { return; }
    let mut pty = session();
    let log_path = pty.scratch_path().join("last_output.log");
    assert!(!log_path.exists(), "startup must not publish last_output.log");
    #[cfg(target_os = "linux")]
    let large_cmd = "i=0; while [ $i -lt 160 ]; do printf 'ROW_%03d\\n' $i; i=$((i+1)); done";
    #[cfg(windows)]
    let large_cmd = "& pwsh.exe -NoProfile -Command '0..159 | ForEach-Object { \"ROW_{0:D3}\" -f $_ }'";
    let outcome = pty.execute(large_cmd, COMMAND, &AtomicBool::new(false)).expect("large command completes");
    let view = match outcome {
        PtyCommandOutcome::Completed { exit_code: 0, output } => output,
        other => panic!("expected exit 0, got {other:?}"),
    };
    assert!(view.pruned);
    assert!(view.lines >= 160);
    assert!(view.text.len() <= 16_384);
    assert!(view.text.contains("ROW_000"));
    assert!(view.text.contains("ROW_159"));
    assert!(!view.text.contains("ROW_080"));
    let raw = std::fs::read(&log_path).expect("last_output.log published");
    assert_eq!(raw.len() as u64, view.raw_bytes);
    let raw_text = String::from_utf8_lossy(&raw);
    assert!(raw_text.contains("ROW_080"));

    #[cfg(target_os = "linux")]
    let fail_cmd = "printf 'PARTIAL_FAILURE\\n'; sleep 5";
    #[cfg(windows)]
    let fail_cmd = "[Console]::WriteLine(('PARTIAL_' + 'FAILURE')); Start-Sleep -Seconds 30";
    let _ = pty.execute(fail_cmd, Duration::from_secs(1), &AtomicBool::new(false)).expect_err("timed-out command fails");
    assert_eq!(std::fs::read(&log_path).unwrap(), raw, "failed command must not overwrite prior last_output.log");
}
