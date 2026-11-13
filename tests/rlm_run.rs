use ferrocore_driver::{read_session_log, LogRecord};
use std::{fs, io::{Read, Write}, net::TcpListener, path::{Path, PathBuf}, process::{Command, Output}, sync::atomic::{AtomicU64, Ordering}, thread, time::{Duration, Instant}};

const KEY: &str = "rlm-run-test-secret";
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        loop {
            let path = std::env::temp_dir().join(format!("ferrocore-rlm-run-{}-{}", std::process::id(), NEXT_TEMP.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&path) {
                Ok(()) => {
                    #[cfg(windows)]
                    set_private_acl(&path);
                    return Self(path);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("temporary directory: {e}"),
            }
        }
    }
}
impl Drop for TestDir { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }

#[cfg(windows)]
fn set_private_acl(path: &Path) {
    use std::{ffi::c_void, ptr};
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
    let sid = unsafe {
        let mut token = ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), 8, &mut token), 0);
        let mut needed = 0;
        GetTokenInformation(token, 1, ptr::null_mut(), 0, &mut needed);
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
    assert!(Command::new("icacls").arg(path).args(["/inheritance:r", "/grant:r"])
        .arg(format!("*{sid}:(OI)(CI)F"))
        .args(["*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"])
        .status().expect("icacls required").success());
}

fn sse(content: &str, usage: bool, finish: &str) -> String {
    let encoded = format!("\"{}\"", content.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n"));
    let usage = if usage { ",\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}" } else { "" };
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":{encoded}}},\"finish_reason\":\"{finish}\"}}]{usage}}}\n\ndata: [DONE]\n\n")
}

fn run(responses: Vec<String>, max_tokens: u64, max_bytes: usize) -> (Output, Vec<LogRecord>, Vec<String>) {
    let dir = TestDir::new();
    let logs = dir.0.join("logs");
    fs::create_dir(&logs).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        for response in responses {
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                    Err(e) => panic!("server accept: {e}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).unwrap();
                assert_ne!(n, 0);
                request.extend_from_slice(&buf[..n]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers.lines().find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                    }).unwrap();
                    if request.len() >= end + 4 + length { break; }
                }
            }
            requests.push(String::from_utf8(request).unwrap());
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", response.len());
            socket.write_all(header.as_bytes()).unwrap();
            let _ = socket.write_all(response.as_bytes());
        }
        requests
    });
    let config = format!("active_loop = \"rlm\"\n[llm]\nbase_url = \"http://{address}/v1\"\nmodel = \"test\"\napi_key_env = \"FERROCORE_RLM_RUN_TEST_KEY\"\nmax_retries = 0\nretry_base_ms = 1\nretry_cap_ms = 1\n[budget]\nmax_turns = 3\nmax_total_tokens = {max_tokens}\nmax_response_bytes = {max_bytes}\n[pty]\nstartup_timeout_secs = 10\nstep_timeout_secs = 10\noutput_view_bytes = 16384\nscratch_entries = 20\n[session]\nsession_timeout_secs = 20\nlog_dir = \"logs\"\n");
    let config_path = dir.0.join("config.toml");
    fs::write(&config_path, config).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ferrocore"));
    command.current_dir(&dir.0).args(["run", "--config"]).arg(&config_path).arg("test task")
        .env("FERROCORE_RLM_RUN_TEST_KEY", KEY);
    #[cfg(windows)]
    command.env("TEMP", &dir.0).env("TMP", &dir.0);
    let output = command.output().unwrap();
    let requests = server.join().unwrap();
    let entries: Vec<_> = fs::read_dir(&logs).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(entries.len(), 1);
    let path = entries[0].join("session.jsonl");
    let text = fs::read_to_string(&path).unwrap();
    assert!(!text.contains(KEY));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(KEY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(KEY));
    (output, read_session_log(path).unwrap(), requests)
}

#[test]
fn session_deadline_overflow_has_no_log_or_network_side_effects() {
    // `Instant` representable range is platform-specific. Only exercise the
    // overflow case on platforms where this TOML value actually overflows.
    if std::time::Instant::now().checked_add(Duration::from_secs(i64::MAX as u64)).is_some() {
        return;
    }
    let dir = TestDir::new();
    let logs = dir.0.join("logs");
    fs::create_dir(&logs).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    // TOML integers are signed 64-bit values; this is the largest valid one.
    let config = format!("active_loop = \"rlm\"\n[llm]\nbase_url = \"http://{address}/v1\"\nmodel = \"test\"\napi_key_env = \"FERROCORE_RLM_RUN_TEST_KEY\"\nmax_retries = 0\nretry_base_ms = 1\nretry_cap_ms = 1\n[budget]\nmax_turns = 3\nmax_total_tokens = 1000\nmax_response_bytes = 1048576\n[pty]\nstartup_timeout_secs = 10\nstep_timeout_secs = 10\noutput_view_bytes = 16384\nscratch_entries = 20\n[session]\nsession_timeout_secs = 9223372036854775807\nlog_dir = \"logs\"\n");
    let config_path = dir.0.join("config.toml");
    fs::write(&config_path, config).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ferrocore"));
    command.current_dir(&dir.0).args(["run", "--config"]).arg(&config_path).arg("test task")
        .env("FERROCORE_RLM_RUN_TEST_KEY", KEY)
        .env("NO_PROXY", "127.0.0.1").env("no_proxy", "127.0.0.1")
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    command.env("TEMP", &dir.0).env("TMP", &dir.0);
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let timed_out = loop {
        if child.try_wait().unwrap().is_some() { break false; }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            break true;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = child.wait_with_output().unwrap();
    assert!(!timed_out, "CLI did not exit before timeout");
    assert!(!output.status.success(), "CLI unexpectedly succeeded");
    assert!(String::from_utf8_lossy(&output.stderr).contains("configured timeout exceeds the supported deadline range"),
        "unexpected stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(fs::read_dir(&logs).unwrap().next().is_none(), "session log directory is not empty");
    match listener.accept() {
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {},
        Ok(_) => panic!("network connection started"),
        Err(e) => panic!("checking network listener: {e}"),
    }
}

fn kinds(records: &[LogRecord]) -> Vec<&str> {
    records.iter().map(|record| record.payload.kind.as_str()).collect()
}

#[test]
fn two_turns_execute_shell_and_record_order() {
    let shell = if cfg!(windows) { "<sh>\nWrite-Output ferrocore_e2e_marker\n</sh>" } else { "<sh>\necho ferrocore_e2e_marker\n</sh>" };
    let (output, records, requests) = run(vec![
        sse(shell, true, "stop"),
        sse("<final>\ncompleted\n</final>", true, "stop"),
    ], 1000, 1048576);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "completed");
    assert_eq!(kinds(&records), ["session_start", "llm_output", "pty_exec", "pty_result", "llm_output", "final", "session_end"]);
    for pair in records.windows(2) { assert_eq!(pair[1].parent_id, Some(pair[0].id)); }
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("ferrocore_e2e_marker"));
    assert!(records[3].payload.data.to_string().contains("ferrocore_e2e_marker"));
}

#[test]
fn inline_closing_tag_is_literal_in_shell_command_through_cli() {
    let command = if cfg!(windows) { "Write-Output 'ferrocore_inline_</sh>_marker'" } else { "printf '%s\\n' 'ferrocore_inline_</sh>_marker'" };
    let shell = format!("<sh>\n{command}\n</sh>");
    let (output, records, requests) = run(vec![
        sse(&shell, true, "stop"),
        sse("<final>\ncompleted\n</final>", true, "stop"),
    ], 1000, 1048576);
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "completed");
    assert_eq!(kinds(&records), ["session_start", "llm_output", "pty_exec", "pty_result", "llm_output", "final", "session_end"]);
    assert_eq!(records[2].payload.data["command"], command);
    assert_eq!(records[3].payload.data["exit_code"], 0);
    assert!(records[3].payload.data["output"].as_str().unwrap().contains("ferrocore_inline_</sh>_marker"));
    assert_eq!(requests.len(), 2);
    assert!(requests[1].contains("ferrocore_inline_</sh>_marker"));
}

#[test]
fn inline_closing_tag_without_closing_line_never_executes_through_cli() {
    let command = if cfg!(windows) { "Write-Output 'ferrocore_inline_</sh>_marker'" } else { "printf '%s\\n' 'ferrocore_inline_</sh>_marker'" };
    let malformed = sse(&format!("<sh>\n{command}"), true, "stop");
    let (output, records, requests) = run(vec![malformed.clone(), malformed.clone(), malformed], 1000, 1048576);
    assert!(!output.status.success());
    assert_eq!(requests.len(), 3);
    assert_eq!(kinds(&records), ["session_start", "llm_output", "error", "llm_output", "error", "llm_output", "error", "error", "session_end"]);
    for index in [2, 4, 6] { assert_eq!(records[index].payload.data["reason"], "malformed_response"); }
    assert!(!records.iter().any(|record| record.payload.kind == "pty_exec"));
}

#[test]
fn missing_usage_and_budget_limit_never_execute_command() {
    for (usage, max_tokens, reason) in [(false, 1000, "usage_unavailable"), (true, 10, "budget_exceeded")] {
        let (output, records, _) = run(vec![sse("<sh>\necho must_not_execute\n</sh>", usage, "stop")], max_tokens, 1048576);
        assert!(!output.status.success());
        assert_eq!(kinds(&records), ["session_start", "llm_output", "error", "error", "session_end"]);
        assert_eq!(records[2].payload.data["reason"], reason);
        assert_eq!(records[2].parent_id, Some(records[1].id));
    }
}

#[test]
fn length_and_malformed_response_never_execute_command() {
    let (output, records, _) = run(vec![sse("<sh>\necho must_not_execute\n</sh>", true, "length")], 1000, 1048576);
    assert!(!output.status.success());
    assert_eq!(kinds(&records), ["session_start", "llm_output", "error", "error", "session_end"]);
    assert_eq!(records[2].payload.data["reason"], "finish_reason_length");
    for pair in records.windows(2) { assert_eq!(pair[1].parent_id, Some(pair[0].id)); }
    let malformed = sse("<sh>\necho must_not_execute", true, "stop");
    let (output, records, _) = run(vec![malformed.clone(), malformed.clone(), malformed], 1000, 1048576);
    assert!(!output.status.success());
    assert_eq!(kinds(&records), ["session_start", "llm_output", "error", "llm_output", "error", "llm_output", "error", "error", "session_end"]);
    for index in [2, 4, 6] { assert_eq!(records[index].payload.data["reason"], "malformed_response"); }
}

#[test]
fn embedded_tags_never_execute_shell_through_cli() {
    let responses = [
        "prefix <sh>\n<sh>\necho must_not_execute\n</sh>",
        "<sh>\necho '</sh>'",
        "<sh>\necho must_not_execute\n</sh>\nsuffix <final>",
    ];
    let (output, records, requests) = run(responses.into_iter().map(|text| sse(text, true, "stop")).collect(), 1000, 1048576);
    assert!(!output.status.success());
    assert_eq!(requests.len(), 3);
    assert_eq!(kinds(&records), ["session_start", "llm_output", "error", "llm_output", "error", "llm_output", "error", "error", "session_end"]);
    for index in [2, 4, 6] { assert_eq!(records[index].payload.data["reason"], "malformed_response"); }
    assert!(!records.iter().any(|record| record.payload.kind == "pty_exec"));
}

#[test]
fn response_byte_limit_blocks_execution() {
    let (output, records, _) = run(vec![sse("<sh>\necho must_not_execute\n</sh>", true, "stop")], 1000, 32);
    assert!(!output.status.success());
    assert_eq!(kinds(&records), ["session_start", "llm_output", "error", "error", "session_end"]);
    assert_eq!(records[2].payload.data["reason"], "response_limit_exceeded");
}

#[derive(Clone, Copy)]
enum TimeoutResponse { HangingSse, RetryAfter }

fn run_step_timeout(response: TimeoutResponse) -> (Output, Vec<LogRecord>, usize, Duration) {
    use std::sync::{Arc, atomic::AtomicBool};

    let dir = TestDir::new();
    let logs = dir.0.join("logs");
    fs::create_dir(&logs).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = stop.clone();
    let server = thread::spawn(move || {
        let mut requests = 0;
        let mut hanging = Vec::new();
        loop {
            match listener.accept() {
                Ok((mut socket, _)) => {
                    requests += 1;
                    socket.set_nonblocking(false).unwrap();
                    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut request = Vec::new();
                    let mut buf = [0u8; 4096];
                    loop {
                        let n = socket.read(&mut buf).unwrap();
                        assert_ne!(n, 0);
                        request.extend_from_slice(&buf[..n]);
                        if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&request[..end]);
                            let length = headers.lines().find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().unwrap())
                            }).unwrap();
                            if request.len() >= end + 4 + length { break; }
                        }
                    }
                    match response {
                        TimeoutResponse::HangingSse => {
                            let partial = "data: {\"choices\":[{\"delta\":{\"content\":\"<sh>\\necho must_not_execute\\n</sh>\"}}]}\n\n";
                            let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                            socket.write_all(format!("{header}{:x}\r\n{partial}\r\n", partial.len()).as_bytes()).unwrap();
                            hanging.push(socket);
                        }
                        TimeoutResponse::RetryAfter => {
                            socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nRetry-After: 10\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if server_stop.load(Ordering::Acquire) { break; }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("server accept: {e}"),
            }
        }
        requests
    });
    let config = format!("active_loop = \"rlm\"\n[llm]\nbase_url = \"http://{address}/v1\"\nmodel = \"test\"\napi_key_env = \"FERROCORE_RLM_RUN_TEST_KEY\"\nmax_retries = 3\nretry_base_ms = 1\nretry_cap_ms = 1\n[budget]\nmax_turns = 3\nmax_total_tokens = 1000\nmax_response_bytes = 1048576\n[pty]\nstartup_timeout_secs = 10\nstep_timeout_secs = 2\noutput_view_bytes = 16384\nscratch_entries = 20\n[session]\nsession_timeout_secs = 60\nlog_dir = \"logs\"\n");
    let config_path = dir.0.join("config.toml");
    fs::write(&config_path, config).unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_ferrocore"));
    command.current_dir(&dir.0).args(["run", "--config"]).arg(&config_path).arg("test task")
        .env("FERROCORE_RLM_RUN_TEST_KEY", KEY)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
    #[cfg(windows)]
    command.env("TEMP", &dir.0).env("TMP", &dir.0);
    let start = Instant::now();
    let mut child = command.spawn().unwrap();
    let timed_out = loop {
        if child.try_wait().unwrap().is_some() { break false; }
        if start.elapsed() >= Duration::from_secs(20) {
            child.kill().unwrap();
            break true;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let output = child.wait_with_output().unwrap();
    let elapsed = start.elapsed();
    stop.store(true, Ordering::Release);
    let requests = server.join().unwrap();
    assert!(!timed_out, "CLI did not respect the step timeout");
    let entries: Vec<_> = fs::read_dir(&logs).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(entries.len(), 1);
    let path = entries[0].join("session.jsonl");
    assert!(!fs::read_to_string(&path).unwrap().contains(KEY));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(KEY));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(KEY));
    (output, read_session_log(path).unwrap(), requests, elapsed)
}

#[test]
fn step_timeout_interrupts_hanging_sse_without_executing_partial_command() {
    let (output, records, requests, elapsed) = run_step_timeout(TimeoutResponse::HangingSse);
    assert!(!output.status.success());
    assert!(elapsed < Duration::from_secs(20));
    assert_eq!(requests, 1);
    assert_eq!(kinds(&records), ["session_start", "llm_output", "error", "error", "session_end"]);
    assert_eq!(records[1].payload.data["partial"], true);
    assert!(records[1].payload.data["raw_text"].as_str().unwrap().contains("must_not_execute"));
    assert_eq!(records[1].payload.data["truncated_reason"], "timeout");
    assert_eq!(records[2].payload.data["reason"], "timeout");
    assert_eq!(records[3].payload.data["reason"], "session_failed");
    assert_eq!(records[4].payload.data["success"], false);
    for pair in records.windows(2) { assert_eq!(pair[1].parent_id, Some(pair[0].id)); }
}

#[test]
fn step_timeout_prevents_retry_and_command_execution() {
    let (output, records, requests, elapsed) = run_step_timeout(TimeoutResponse::RetryAfter);
    assert!(!output.status.success());
    assert!(elapsed < Duration::from_secs(20));
    assert_eq!(requests, 1);
    assert_eq!(kinds(&records), ["session_start", "error", "error", "session_end"]);
    assert_eq!(records[1].payload.data["reason"], "retry_after_exceeds_timeout");
    assert_eq!(records[2].payload.data["reason"], "session_failed");
    assert_eq!(records[3].payload.data["success"], false);
    for pair in records.windows(2) { assert_eq!(pair[1].parent_id, Some(pair[0].id)); }
}
