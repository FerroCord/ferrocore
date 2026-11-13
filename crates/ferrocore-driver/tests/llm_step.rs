use ferrocore_driver::budget::{Budget, BudgetConfig, BudgetError};
use ferrocore_driver::llm::{LlmClient, LlmConfig, LlmErrorKind};
use ferrocore_driver::{read_session_log, run_llm_step, LlmStepError, SessionLogWriter};
use ferrocore_types::Message;
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        loop {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("ferrocore-llm-step-{}-{id}", std::process::id()));
            match fs::create_dir(&path) {
                Ok(()) => {
                    #[cfg(windows)]
                    set_private_test_acl(&path);
                    return Self(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create temporary directory: {error}"),
            }
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(windows)]
fn set_private_test_acl(path: &std::path::Path) {
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
    let user_sid = unsafe {
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
    let status = std::process::Command::new("icacls")
        .arg(path)
        .args(["/inheritance:r", "/grant:r"])
        .arg(format!("*{user_sid}:(OI)(CI)F"))
        .args(["*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"])
        .status()
        .expect("icacls required");
    assert!(status.success());
}

async fn serve_once(body: &'static str, truncate_before_done: bool) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let header = if truncate_before_done {
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n".to_string()
        } else {
            format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())
        };
        sock.write_all(header.as_bytes()).await.unwrap();
        sock.write_all(body.as_bytes()).await.unwrap();
    });
    addr
}

fn client_for(addr: std::net::SocketAddr, max_response_bytes: usize) -> LlmClient {
    LlmClient::new(LlmConfig {
        base_url: format!("http://{addr}/v1"),
        model: "test-model".into(),
        api_key: "step-secret-key".into(),
        max_retries: 2,
        retry_base: Duration::from_millis(5),
        retry_cap: Duration::from_millis(20),
        max_response_bytes,
    })
    .unwrap()
}

fn messages() -> [Message; 1] {
    [Message { role: "user".into(), content: "run check using step-secret-key".into() }]
}

#[tokio::test(flavor = "current_thread")]
async fn missing_usage_records_response_and_stops_before_execution() {
    let dir = TestDir::new();
    let log = SessionLogWriter::open(&dir.0, "step-secret-key").await.unwrap();
    let root = log.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"<sh>\\necho step-secret-key\\n</sh>\"}}]}\n\ndata: [DONE]\n\n";
    let addr = serve_once(sse, false).await;
    let client = client_for(addr, 1024 * 1024);
    let mut budget = Budget::new(BudgetConfig {
        max_turns: 5,
        max_total_tokens: 1000,
        max_cost_micros_usd: None,
        input_cost_per_million_micros: None,
        output_cost_per_million_micros: None,
    })
    .unwrap();

    let err = run_llm_step(&client, &mut budget, &log, root, &messages(), &CancellationToken::new(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, LlmStepError::Budget { error: BudgetError::UsageUnavailable, .. }));
    log.close().await.unwrap();

    let records = read_session_log(log.path()).unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].payload.kind, "llm_output");
    assert_eq!(records[1].payload.data["raw_text"], "<sh>\necho [REDACTED]\n</sh>");
    assert_eq!(records[2].payload.kind, "error");
    assert_eq!(records[2].payload.data["reason"], "usage_unavailable");
    assert!(!fs::read_to_string(log.path()).unwrap().contains("step-secret-key"));
}

#[tokio::test(flavor = "current_thread")]
async fn exceeding_budget_records_response_and_blocks_next_turn() {
    let dir = TestDir::new();
    let log = SessionLogWriter::open(&dir.0, "step-secret-key").await.unwrap();
    let root = log.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
    let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"<sh>\\necho hi\\n</sh>\"}}]}\n\n\
               data: {\"choices\":[],\"usage\":{\"prompt_tokens\":60,\"completion_tokens\":50,\"total_tokens\":110}}\n\n\
               data: [DONE]\n\n";
    let addr = serve_once(sse, false).await;
    let client = client_for(addr, 1024 * 1024);
    let mut budget = Budget::new(BudgetConfig {
        max_turns: 5,
        max_total_tokens: 100,
        max_cost_micros_usd: Some(500),
        input_cost_per_million_micros: Some(1_000_000),
        output_cost_per_million_micros: Some(2_000_000),
    })
    .unwrap();

    let err = run_llm_step(&client, &mut budget, &log, root, &messages(), &CancellationToken::new(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, LlmStepError::Budget { error: BudgetError::BudgetExceeded, .. }));
    log.close().await.unwrap();

    let records = read_session_log(log.path()).unwrap();
    assert_eq!(records.len(), 3);
    assert_eq!(records[1].payload.kind, "llm_output");
    assert_eq!(records[2].payload.data["reason"], "budget_exceeded");
}

#[tokio::test(flavor = "current_thread")]
async fn response_limit_and_stream_interruption_persist_partial_and_reason() {
    let dir = TestDir::new();
    let log = SessionLogWriter::open(&dir.0, "step-secret-key").await.unwrap();
    let root = log.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
    let limit_sse = "data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n\
                     data: {\"choices\":[{\"delta\":{\"content\":\"second-chunk-exceeds-second-chunk-exceeds-second-chunk-exceeds-second-chunk-exceeds\"}}]}\n\n";
    let addr = serve_once(limit_sse, true).await;
    let client = client_for(addr, 80);
    let mut budget = Budget::new(BudgetConfig {
        max_turns: 5,
        max_total_tokens: 1000,
        max_cost_micros_usd: None,
        input_cost_per_million_micros: None,
        output_cost_per_million_micros: None,
    })
    .unwrap();

    let err = run_llm_step(&client, &mut budget, &log, root, &messages(), &CancellationToken::new(), None)
        .await
        .unwrap_err();
    let last_id = match &err {
        LlmStepError::Llm { last_record_id: Some(id), error } => {
            assert_eq!(error.kind, LlmErrorKind::ResponseLimitExceeded);
            assert_eq!(error.partial.text, "first");
            *id
        }
        other => panic!("unexpected outcome: {other:?}"),
    };

    // A stream truncated after first delta records partial output and stops without retry.
    let partial_sse = "data: {\"choices\":[{\"delta\":{\"content\":\"partial-only\"}}]}\n\n";
    let addr2 = serve_once(partial_sse, true).await;
    let client2 = client_for(addr2, 1024 * 1024);
    let mut budget2 = Budget::new(BudgetConfig {
        max_turns: 5,
        max_total_tokens: 1000,
        max_cost_micros_usd: None,
        input_cost_per_million_micros: None,
        output_cost_per_million_micros: None,
    })
    .unwrap();
    let err2 = run_llm_step(&client2, &mut budget2, &log, last_id, &messages(), &CancellationToken::new(), None)
        .await
        .unwrap_err();
    match &err2 {
        LlmStepError::Llm { error, .. } => {
            assert_eq!(error.kind, LlmErrorKind::StreamInterrupted);
            assert_eq!(error.partial.text, "partial-only");
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
    log.close().await.unwrap();

    let records = read_session_log(log.path()).unwrap();
    assert_eq!(records.len(), 5);
    assert_eq!(records[1].payload.kind, "llm_output");
    assert_eq!(records[1].payload.data["raw_text"], "first");
    assert_eq!(records[1].payload.data["truncated_reason"], "response_limit_exceeded");
    assert_eq!(records[2].payload.data["reason"], "response_limit_exceeded");
    assert_eq!(records[3].payload.kind, "llm_output");
    assert_eq!(records[3].payload.data["raw_text"], "partial-only");
    assert_eq!(records[4].payload.data["reason"], "stream_interrupted");
}
