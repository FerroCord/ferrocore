//! Append-only session logs. A successful append acknowledges a flushed line, not
//! execution of the action described by that line or power-loss durability.

pub mod budget;
pub mod llm;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fmt;
use std::fs::File;
#[cfg(unix)]
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;

/// Errors deliberately omit paths, input and underlying error messages, which may
/// contain credentials. A failed writer cannot accept further operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogError {
    UnsupportedPlatform,
    InsecurePermissions,
    Io,
    InvalidRecord,
    DuplicateId,
    MultipleOrMissingRoot,
    DanglingParentId,
    TruncatedTailLine,
    CorruptLine,
    AcknowledgementLost,
    Closed,
}

impl fmt::Display for LogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedPlatform => "secure session log permissions cannot be verified on this platform",
            Self::InsecurePermissions => "session log permissions cannot be verified as secure",
            Self::Io => "session log I/O failed",
            Self::InvalidRecord => "invalid session log record",
            Self::DuplicateId => "duplicate session log record id",
            Self::MultipleOrMissingRoot => "session log must have exactly one root",
            Self::DanglingParentId => "session log parent has not been written",
            Self::TruncatedTailLine => "session log has a truncated final line",
            Self::CorruptLine => "session log contains a corrupt line",
            Self::AcknowledgementLost => "session log acknowledgement lost; write outcome is uncertain",
            Self::Closed => "session log writer is closed",
        })
    }
}
impl std::error::Error for LogError {}

/// The exact six-field on-disk record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LogRecord {
    pub id: Uuid,
    #[serde(deserialize_with = "deserialize_parent")]
    pub parent_id: Option<Uuid>,
    pub agent_depth: u32,
    pub loop_type: String,
    pub timestamp: String,
    pub payload: LogPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LogPayload {
    pub v: u32,
    pub kind: String,
    pub data: Value,
}

fn v7(id: Uuid) -> bool { id.get_version_num() == 7 && id.get_variant() == uuid::Variant::RFC4122 }


fn deserialize_parent<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Uuid>, D::Error> {
    Option::<Uuid>::deserialize(deserializer)
}
fn validate(record: &LogRecord, ids: &mut HashSet<Uuid>) -> Result<(), LogError> {
    if !v7(record.id) || record.agent_depth != 0 || record.loop_type != "rlm" || record.payload.kind.is_empty()
        || record.payload.v != 1 || !record.payload.data.is_object()
        || DateTime::parse_from_rfc3339(&record.timestamp)
            .map(|date| date.offset().local_minus_utc() != 0 || !record.timestamp.ends_with('Z'))
            .unwrap_or(true)
    { return Err(LogError::InvalidRecord); }
    if ids.contains(&record.id) { return Err(LogError::DuplicateId); }
    match record.parent_id {
        None if !ids.is_empty() || record.payload.kind != "session_start" => return Err(LogError::MultipleOrMissingRoot),
        Some(_) if ids.is_empty() => return Err(LogError::MultipleOrMissingRoot),
        Some(parent) if !ids.contains(&parent) => return Err(LogError::DanglingParentId),
        Some(_) if record.payload.kind == "session_start" => return Err(LogError::MultipleOrMissingRoot),
        _ => {}
    }
    ids.insert(record.id);
    Ok(())
}

/// Reads and validates a complete JSONL file without modifying it. A missing LF
/// on the last line is reported separately, even when that line is valid JSON.
pub fn read_session_log(path: impl AsRef<Path>) -> Result<Vec<LogRecord>, LogError> {
    let file = File::open(path).map_err(|_| LogError::Io)?;
    let mut reader = BufReader::new(file);
    let mut records = Vec::new();
    let mut ids = HashSet::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if reader.read_until(b'\n', &mut line).map_err(|_| LogError::Io)? == 0 { break; }
        if line.last() != Some(&b'\n') { return Err(LogError::TruncatedTailLine); }
        let record: LogRecord = serde_json::from_slice(&line).map_err(|_| LogError::CorruptLine)?;
        validate(&record, &mut ids)?;
        records.push(record);
    }
    if records.is_empty() { return Err(LogError::MultipleOrMissingRoot); }
    Ok(records)
}

fn redact(value: &mut Value, secret: &str) -> Result<(), LogError> {
    match value {
        Value::String(s) => { if !secret.is_empty() { *s = s.replace(secret, "[REDACTED]"); } }
        Value::Array(values) => {
            for value in values { redact(value, secret)?; }
        }
        Value::Object(map) => {
            let old = std::mem::take(map);
            for (key, mut value) in old {
                redact(&mut value, secret)?;
                let key = if secret.is_empty() { key } else { key.replace(secret, "[REDACTED]") };
                if map.insert(key, value).is_some() { return Err(LogError::InvalidRecord); }
            }
        }
        _ => {}
    }
    Ok(())
}

fn contains_secret(value: &Value, secret: &str) -> bool {
    match value {
        Value::String(s) => s.contains(secret),
        Value::Array(values) => values.iter().any(|value| contains_secret(value, secret)),
        Value::Object(map) => map.iter().any(|(key, value)| key.contains(secret) || contains_secret(value, secret)),
        _ => false,
    }
}
trait LogFile: Write {
    fn sync_data(&self) -> std::io::Result<()>;
}

impl LogFile for File {
    fn sync_data(&self) -> std::io::Result<()> { File::sync_data(self) }
}

#[cfg(test)]
mod writer_unit;


type Reply<T> = oneshot::Sender<Result<T, LogError>>;
enum Command {
    Append { parent_id: Option<Uuid>, agent_depth: u32, loop_type: String, kind: String, data: Value, reply: Reply<Uuid>, consumed: oneshot::Receiver<()> },
    Sync(Reply<()>),
    Close(Reply<()>),
}

/// Cloneable producer handle for one bounded, single-owner writer task.
/// Callers must await every append before performing its associated side effect.
#[derive(Clone)]
pub struct SessionLogWriter {
    sender: mpsc::Sender<Command>,
    session_id: Uuid,
    path: PathBuf,
}

impl SessionLogWriter {
    /// Creates `<log_dir>/<uuid-v7>/session.jsonl` exclusively. The log directory
    /// must already exist. Windows verifies inherited ACLs permit only the
    /// process token user, SYSTEM and local Administrators; unknown ACEs fail closed.
    /// The caller must trust the parent path and its ancestors: replacement or
    /// reparse points in that path are not prevented. Privileged administrators,
    /// SYSTEM and later ACL changes are outside this check. Impersonated thread
    /// tokens are not used. Requires a Tokio runtime.
    pub async fn open(log_dir: impl AsRef<Path>, secret: impl Into<String>) -> Result<Self, LogError> {
        let secret = secret.into();
        #[cfg(not(any(unix, windows)))]
        { let _ = (log_dir, secret); return Err(LogError::UnsupportedPlatform); }
        #[cfg(windows)]
        {
            let (session_id, dir, file) = windows_security::create(log_dir.as_ref())?;
            let path = dir.join("session.jsonl");
            file.sync_data().map_err(|_| LogError::Io)?;
            let (sender, receiver) = mpsc::channel(128);
            tokio::spawn(writer_task(file, receiver, secret));
            Ok(Self { sender, session_id, path })
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
            let (session_id, dir) = loop {
                let id = Uuid::now_v7();
                let dir = log_dir.as_ref().join(id.to_string());
                match fs::DirBuilder::new().mode(0o700).create(&dir) {
                    Ok(()) => break (id, dir),
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(_) => return Err(LogError::Io),
                }
            };
            let path = dir.join("session.jsonl");
            let file = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|_| LogError::Io)?;
            // Verify actual modes, including an unexpectedly restrictive umask.
            if fs::metadata(&dir).map_err(|_| LogError::Io)?.permissions().mode() & 0o777 != 0o700
                || file.metadata().map_err(|_| LogError::Io)?.permissions().mode() & 0o777 != 0o600 {
                return Err(LogError::Io);
            }
            file.sync_data().map_err(|_| LogError::Io)?;
            let (sender, receiver) = mpsc::channel(128);
            tokio::spawn(writer_task(file, receiver, secret));
            Ok(Self { sender, session_id, path })
        }
    }

    pub fn session_id(&self) -> Uuid { self.session_id }
    pub fn path(&self) -> &Path { &self.path }

    /// The first append must be a `session_start` root. Subsequent parents must
    /// refer to already acknowledged records. Data must be a JSON object.
    /// If the append future is cancelled after submission, its outcome is unknown:
    /// do not retry it; stop the session safely. Cancellation can race with writing.
    pub async fn append(&self, parent_id: Option<Uuid>, agent_depth: u32, loop_type: impl Into<String>, kind: impl Into<String>, data: Value) -> Result<Uuid, LogError> {
        let (reply, response) = oneshot::channel();
        let (consumed, receipt) = oneshot::channel();
        self.sender.send(Command::Append { parent_id, agent_depth, loop_type: loop_type.into(), kind: kind.into(), data, reply, consumed: receipt }).await.map_err(|_| LogError::Closed)?;
        let result = response.await.map_err(|_| LogError::Closed)?;
        let _ = consumed.send(());
        result
    }

    /// Durably requests a turn boundary; sync_data is not an absolute power-loss guarantee.
    pub async fn sync_turn(&self) -> Result<(), LogError> {
        let (reply, response) = oneshot::channel();
        self.sender.send(Command::Sync(reply)).await.map_err(|_| LogError::Closed)?;
        response.await.map_err(|_| LogError::Closed)?
    }

    /// Syncs and stops the shared writer. All clones become closed.
    pub async fn close(&self) -> Result<(), LogError> {
        let (reply, response) = oneshot::channel();
        self.sender.send(Command::Close(reply)).await.map_err(|_| LogError::Closed)?;
        response.await.map_err(|_| LogError::Closed)?
    }
}

async fn writer_task(mut file: impl LogFile, mut receiver: mpsc::Receiver<Command>, secret: String) {
    let mut ids = HashSet::new();
    let mut failure: Option<LogError> = None;
    while let Some(command) = receiver.recv().await {
        match command {
            Command::Append { parent_id, agent_depth, loop_type, kind, data, reply, consumed } => {
                if reply.is_closed() { continue; }
                let result = if let Some(error) = &failure { Err(error.clone()) } else {
                    let record = LogRecord { id: Uuid::now_v7(), parent_id, agent_depth, loop_type, timestamp: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true), payload: LogPayload { v: 1, kind, data } };
                    (|| {
                        validate(&record, &mut ids)?;
                        ids.remove(&record.id);
                        let mut value = serde_json::to_value(record).map_err(|_| LogError::InvalidRecord)?;
                        redact(&mut value, &secret)?;
                        if !secret.is_empty() && contains_secret(&value, &secret) {
                            return Err(LogError::InvalidRecord);
                        }
                        let record: LogRecord = serde_json::from_value(value).map_err(|_| LogError::InvalidRecord)?;
                        validate(&record, &mut ids).map_err(|_| LogError::InvalidRecord)?;
                        let mut line = serde_json::to_vec(&record).map_err(|_| LogError::InvalidRecord)?;
                        line.push(b'\n');
                        if !secret.is_empty() && line.windows(secret.len()).any(|window| window == secret.as_bytes()) {
                            return Err(LogError::InvalidRecord);
                        }
                        file.write_all(&line).and_then(|()| file.flush()).map_err(|_| LogError::Io)?;
                        if record.parent_id.is_none() { file.sync_data().map_err(|_| LogError::Io)?; }
                        Ok(record.id)
                    })()
                };
                if let Err(error) = &result { failure = Some(error.clone()); }
                let delivered = reply.send(result).is_ok();
                if (!delivered || consumed.await.is_err()) && failure.is_none() {
                    failure = Some(LogError::AcknowledgementLost);
                }
            }
            Command::Sync(reply) => {
                let result = failure.clone().map_or_else(|| file.sync_data().map_err(|_| LogError::Io), Err);
                if let Err(error) = &result { failure = Some(error.clone()); }
                let _ = reply.send(result);
            }
            Command::Close(reply) => {
                let result = failure.clone().map_or_else(|| file.sync_data().map_err(|_| LogError::Io), Err);
                let _ = reply.send(result);
                break;
            }
        }
    }
}

// Windows objects inherit their parent's DACL and are checked before writing.
// Unknown ACEs or owners and unverifiable security information fail closed.
#[cfg(windows)]
mod windows_security {
    use super::{File, LogError, Path, PathBuf, Uuid};
    use std::ffi::c_void;
    use std::os::windows::{ffi::OsStrExt, io::{AsRawHandle, FromRawHandle}};
    use std::ptr::{null, null_mut};

    type Handle = *mut c_void;
    type Sid = *mut c_void;
    #[repr(C)] struct TokenUser { sid: Sid, attributes: u32 }
    #[repr(C)] struct AclSize { count: u32, used: u32, free: u32 }
    #[repr(C)] struct AceHeader { kind: u8, flags: u8, size: u16 }
    #[repr(C)] struct AllowAce { header: AceHeader, mask: u32, sid_start: u32 }
    #[repr(C)] struct FileInfo { attributes: u32, times: [u32; 6], volume: u32, size_high: u32, size_low: u32, links: u32, index_high: u32, index_low: u32 }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(token: Handle, class: u32, data: *mut c_void, length: u32, needed: *mut u32) -> i32;
        fn CreateWellKnownSid(kind: u32, domain: Sid, sid: Sid, size: *mut u32) -> i32;
        fn EqualSid(a: Sid, b: Sid) -> i32;
        fn GetSecurityInfo(handle: Handle, kind: u32, info: u32, owner: *mut Sid, group: *mut Sid, dacl: *mut *mut c_void, sacl: *mut *mut c_void, descriptor: *mut *mut c_void) -> u32;
        fn GetAclInformation(acl: *mut c_void, info: *mut c_void, length: u32, class: u32) -> i32;
        fn GetAce(acl: *mut c_void, index: u32, ace: *mut *mut c_void) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
        fn CreateDirectoryW(path: *const u16, attributes: *const c_void) -> i32;
        fn CreateFileW(path: *const u16, access: u32, share: u32, attributes: *const c_void, creation: u32, flags: u32, template: Handle) -> Handle;
        fn GetFileInformationByHandle(handle: Handle, info: *mut FileInfo) -> i32;
        fn GetLastError() -> u32;
    }
    struct Local(*mut c_void);
    impl Drop for Local { fn drop(&mut self) { unsafe { if !self.0.is_null() { LocalFree(self.0); } } } }
    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
    fn wide(path: &Path) -> Vec<u16> { path.as_os_str().encode_wide().chain(Some(0)).collect() }

    unsafe fn verify(handle: Handle, user: Sid, system: Sid, admins: Sid, directory: bool) -> Result<(), LogError> {
        let bad = LogError::InsecurePermissions;
        let mut info: FileInfo = std::mem::zeroed();
        if GetFileInformationByHandle(handle, &mut info) == 0 || info.attributes & 0x400 != 0
            || (info.attributes & 0x10 != 0) != directory { return Err(bad); }
        let (mut owner, mut acl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        let status = GetSecurityInfo(handle, 1, 5, &mut owner, null_mut(), &mut acl, null_mut(), &mut descriptor);
        let _descriptor = Local(descriptor);
        if status != 0 || owner.is_null() || acl.is_null() || descriptor.is_null()
            || (EqualSid(owner, user) == 0 && EqualSid(owner, system) == 0 && EqualSid(owner, admins) == 0) { return Err(bad); }
        let mut size = AclSize { count: 0, used: 0, free: 0 };
        if GetAclInformation(acl, &mut size as *mut _ as _, std::mem::size_of::<AclSize>() as u32, 2) == 0 || size.count == 0 { return Err(bad); }
        for index in 0..size.count {
            let mut ace = null_mut();
            if GetAce(acl, index, &mut ace) == 0 || ace.is_null() { return Err(bad); }
            let header = &*(ace as *const AceHeader);
            if header.kind != 0 || header.flags & !0x1f != 0 || header.size < 16 { return Err(bad); }
            let ace = &*(ace as *const AllowAce);
            let sid = &ace.sid_start as *const u32 as Sid;
            let sid_bytes = sid as *const u8;
            let sid_len = 8usize + (*sid_bytes.add(1) as usize) * 4;
            if *sid_bytes != 1 || sid_len > header.size as usize - 8 { return Err(bad); }
            if EqualSid(sid, user) == 0 && EqualSid(sid, system) == 0 && EqualSid(sid, admins) == 0 { return Err(bad); }
        }
        Ok(())
    }

    pub(super) fn create(base: &Path) -> Result<(Uuid, PathBuf, File), LogError> {
        unsafe {
            let bad = LogError::InsecurePermissions;
            let mut token = null_mut();
            if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 { return Err(bad); }
            let token = OwnedHandle(token);
            let mut needed = 0;
            GetTokenInformation(token.0, 1, null_mut(), 0, &mut needed);
            if needed == 0 { return Err(bad); }
            let mut buffer = vec![0usize; (needed as usize + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()];
            if GetTokenInformation(token.0, 1, buffer.as_mut_ptr() as _, needed, &mut needed) == 0 { return Err(bad); }
            let user = (*(buffer.as_ptr() as *const TokenUser)).sid;
            if user.is_null() { return Err(bad); }
            let mut system = [0u32; 17]; let mut admins = [0u32; 17];
            let mut system_len = 68; let mut admins_len = 68;
            if CreateWellKnownSid(22, null_mut(), system.as_mut_ptr() as _, &mut system_len) == 0
                || CreateWellKnownSid(26, null_mut(), admins.as_mut_ptr() as _, &mut admins_len) == 0 { return Err(bad); }
            let (system, admins) = (system.as_mut_ptr() as Sid, admins.as_mut_ptr() as Sid);
            let (id, dir) = loop {
                let id = Uuid::now_v7(); let dir = base.join(id.to_string());
                if CreateDirectoryW(wide(&dir).as_ptr(), null()) != 0 { break (id, dir); }
                if GetLastError() != 183 { return Err(LogError::Io); }
            };
            let dh = CreateFileW(wide(&dir).as_ptr(), 0x00020000, 3, null(), 3, 0x02200000, null_mut());
            if dh as isize == -1 { return Err(bad); }
            let dh = OwnedHandle(dh);
            verify(dh.0, user, system, admins, true)?;
            let path = dir.join("session.jsonl");
            let fh = CreateFileW(wide(&path).as_ptr(), 0x40020000, 1, null(), 1, 0x00200080, null_mut());
            if fh as isize == -1 { return Err(LogError::Io); }
            let file = File::from_raw_handle(fh as _);
            verify(file.as_raw_handle() as _, user, system, admins, false)?;
            verify(dh.0, user, system, admins, true)?;
            Ok((id, dir, file))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmStepOutcome {
    pub last_record_id: Uuid,
    pub response: llm::LlmResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmStepError {
    Budget { last_record_id: Option<Uuid>, error: budget::BudgetError },
    Llm { last_record_id: Option<Uuid>, error: llm::LlmError },
    Log(LogError),
}

impl fmt::Display for LlmStepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Budget { error, .. } => write!(f, "{error}"),
            Self::Llm { error, .. } => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LlmStepError {}

fn budget_reason_code(error: budget::BudgetError) -> &'static str {
    match error {
        budget::BudgetError::InvalidLimit => "invalid_budget_limit",
        budget::BudgetError::MissingPrice => "missing_price",
        budget::BudgetError::InvalidUsage => "invalid_usage",
        budget::BudgetError::UsageUnavailable => "usage_unavailable",
        budget::BudgetError::ArithmeticOverflow => "budget_overflow",
        budget::BudgetError::BudgetExceeded => "budget_exceeded",
    }
}

/// Executes one pre-checked LLM streaming turn and records the response (including
/// bounded partial output on `response_limit_exceeded` / `stream_interrupted`) and
/// any terminal budget/LLM error before returning. Callers must not execute any
/// command when this returns `Err`.
pub async fn run_llm_step(
    client: &llm::LlmClient,
    budget: &mut budget::Budget,
    log: &SessionLogWriter,
    parent_id: Uuid,
    messages: &[ferrocore_types::Message],
    cancel: &tokio_util::sync::CancellationToken,
    deadline: Option<tokio::time::Instant>,
) -> Result<LlmStepOutcome, LlmStepError> {
    if let Err(error) = budget.check_before_turn() {
        let err_id = log
            .append(
                Some(parent_id),
                0,
                "rlm",
                "error",
                serde_json::json!({ "reason": budget_reason_code(error), "message": error.to_string() }),
            )
            .await
            .map_err(LlmStepError::Log)?;
        return Err(LlmStepError::Budget { last_record_id: Some(err_id), error });
    }

    match client.stream_chat(messages, cancel, deadline).await {
        Ok(response) => {
            let llm_id = log
                .append(
                    Some(parent_id),
                    0,
                    "rlm",
                    "llm_output",
                    serde_json::json!({
                        "raw_text": response.text,
                        "usage": response.usage,
                        "finish_reason": response.finish_reason,
                    }),
                )
                .await
                .map_err(LlmStepError::Log)?;
            if let Err(error) = budget.record_response(response.usage) {
                let err_id = log
                    .append(
                        Some(llm_id),
                        0,
                        "rlm",
                        "error",
                        serde_json::json!({ "reason": budget_reason_code(error), "message": error.to_string() }),
                    )
                    .await
                    .map_err(LlmStepError::Log)?;
                return Err(LlmStepError::Budget { last_record_id: Some(err_id), error });
            }
            if cancel.is_cancelled() {
                let cancelled = llm::LlmError { kind: llm::LlmErrorKind::Cancelled, partial: response };
                let err_id = log
                    .append(
                        Some(llm_id),
                        0,
                        "rlm",
                        "error",
                        serde_json::json!({ "reason": cancelled.kind.reason_code(), "message": cancelled.to_string() }),
                    )
                    .await
                    .map_err(LlmStepError::Log)?;
                return Err(LlmStepError::Llm { last_record_id: Some(err_id), error: cancelled });
            }
            if deadline.is_some_and(|end| tokio::time::Instant::now() >= end) {
                let timed_out = llm::LlmError { kind: llm::LlmErrorKind::Timeout, partial: response };
                let err_id = log
                    .append(
                        Some(llm_id),
                        0,
                        "rlm",
                        "error",
                        serde_json::json!({ "reason": timed_out.kind.reason_code(), "message": timed_out.to_string() }),
                    )
                    .await
                    .map_err(LlmStepError::Log)?;
                return Err(LlmStepError::Llm { last_record_id: Some(err_id), error: timed_out });
            }
            Ok(LlmStepOutcome { last_record_id: llm_id, response })
        }
        Err(error) => {
            let mut prev = parent_id;
            if !error.partial.text.is_empty()
                || error.partial.usage != llm::UsageStatus::Unavailable
                || error.partial.finish_reason.is_some()
                || matches!(
                    error.kind,
                    llm::LlmErrorKind::ResponseLimitExceeded | llm::LlmErrorKind::StreamInterrupted
                )
            {
                prev = log
                    .append(
                        Some(prev),
                        0,
                        "rlm",
                        "llm_output",
                        serde_json::json!({
                            "raw_text": error.partial.text,
                            "usage": error.partial.usage,
                            "finish_reason": error.partial.finish_reason,
                            "partial": true,
                            "truncated_reason": error.kind.reason_code(),
                        }),
                    )
                    .await
                    .map_err(LlmStepError::Log)?;
            }
            let _ = budget.record_response(error.partial.usage);
            let err_id = log
                .append(
                    Some(prev),
                    0,
                    "rlm",
                    "error",
                    serde_json::json!({
                        "reason": error.kind.reason_code(),
                        "message": error.to_string(),
                    }),
                )
                .await
                .map_err(LlmStepError::Log)?;
            Err(LlmStepError::Llm { last_record_id: Some(err_id), error })
        }
    }
}
