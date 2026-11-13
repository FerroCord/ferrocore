use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        loop {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "ferrocore-session-log-{}-{id}",
                std::process::id()
            ));
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
        .status().unwrap();
    assert!(status.success(), "could not set private test ACL");
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

use ferrocore_driver::{LogError, SessionLogWriter};

#[cfg(not(any(unix, windows)))]
#[test]
fn unsupported_permissions_fail_before_creating_a_log() {
    let dir = TestDir::new();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let error = match runtime.block_on(SessionLogWriter::open(&dir.0, "secret")) {
        Ok(_) => panic!("unverified permissions must not be accepted"),
        Err(error) => error,
    };
    assert_eq!(error, LogError::UnsupportedPlatform);
    assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use ferrocore_driver::read_session_log;
    use serde_json::{json, Value};
    use std::os::unix::fs::PermissionsExt;
    use uuid::Uuid;

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread().build().unwrap()
    }

    #[test]
    fn writes_acknowledged_private_redacted_tree() {
        let dir = TestDir::new();
        runtime().block_on(async {
            let writer = SessionLogWriter::open(&dir.0, "test-secret").await.unwrap();
            assert_eq!(writer.session_id().get_version_num(), 7);
            let path = writer.path().to_owned();
            assert_eq!(path.parent().unwrap().file_name().unwrap(), writer.session_id().to_string().as_str());
            assert_eq!(fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

            let root = writer.append(None, 0, "rlm", "session_start", json!({"task": "first\nsecond"})).await.unwrap();
            // Acknowledgement means the complete line is already readable.
            assert_eq!(read_session_log(&path).unwrap().len(), 1);
            let left = writer.append(Some(root), 0, "rlm", "pty_exec", json!({"nested": ["prefix test-secret suffix"]})).await.unwrap();
            let right = writer.append(Some(root), 0, "rlm", "pty_result", json!({"output": "line 1\n\"line 2\""})).await.unwrap();
            writer.sync_turn().await.unwrap();
            writer.close().await.unwrap();
            assert_eq!(writer.append(Some(right), 0, "rlm", "error", json!({})).await, Err(LogError::Closed));

            let bytes = fs::read(&path).unwrap();
            assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 3);
            assert!(!String::from_utf8_lossy(&bytes).contains("test-secret"));
            let records = read_session_log(&path).unwrap();
            assert_eq!(records.iter().map(|r| r.id).collect::<Vec<_>>(), vec![root, left, right]);
            assert_eq!(records[0].parent_id, None);
            assert_eq!(records[1].parent_id, Some(root));
            assert_eq!(records[2].parent_id, Some(root));
            assert_eq!(records[1].agent_depth, 0);
            assert_eq!(records[1].payload.data["nested"][0], "prefix [REDACTED] suffix");
            assert_eq!(records[2].payload.data["output"], "line 1\n\"line 2\"");
            for line in bytes.split(|&b| b == b'\n').filter(|line| !line.is_empty()) {
                let object: Value = serde_json::from_slice(line).unwrap();
                assert_eq!(object.as_object().unwrap().len(), 6);
            }
        });
    }

    #[test]
    fn rejects_invalid_parent_and_fails_fast_on_later_writes() {
        let dir = TestDir::new();
        runtime().block_on(async {
            let writer = SessionLogWriter::open(&dir.0, "secret").await.unwrap();
            let root = writer.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
            let before = fs::read(writer.path()).unwrap();
            assert_eq!(writer.append(Some(Uuid::now_v7()), 0, "rlm", "pty_exec", json!({})).await, Err(LogError::DanglingParentId));
            assert_eq!(writer.append(Some(root), 0, "rlm", "pty_exec", json!({})).await, Err(LogError::DanglingParentId));
            assert_eq!(writer.sync_turn().await, Err(LogError::DanglingParentId));
            assert_eq!(fs::read(writer.path()).unwrap(), before);
            assert_eq!(writer.close().await, Err(LogError::DanglingParentId));
        });
    }

    #[test]
    fn reader_reports_truncated_and_corrupt_real_logs_without_repair() {
        let dir = TestDir::new();
        runtime().block_on(async {
            let writer = SessionLogWriter::open(&dir.0, "secret").await.unwrap();
            writer.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
            let path = writer.path().to_owned();
            writer.close().await.unwrap();
            let original = fs::read(&path).unwrap();

            let mut truncated = original.clone();
            truncated.extend_from_slice(b"{\"id\":");
            fs::write(&path, &truncated).unwrap();
            assert_eq!(read_session_log(&path), Err(LogError::TruncatedTailLine));
            assert_eq!(fs::read(&path).unwrap(), truncated);

            let mut corrupt = original;
            corrupt.extend_from_slice(b"not json\n");
            fs::write(&path, &corrupt).unwrap();
            assert_eq!(read_session_log(&path), Err(LogError::CorruptLine));
            assert_eq!(fs::read(&path).unwrap(), corrupt);
        });
    }
    #[test]
    fn reader_rejects_invalid_tree_and_intermediate_corruption_without_repair() {
        let dir = TestDir::new();
        runtime().block_on(async {
            let writer = SessionLogWriter::open(&dir.0, "secret").await.unwrap();
            let root = writer.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
            writer.append(Some(root), 0, "rlm", "pty_exec", json!({})).await.unwrap();
            let path = writer.path().to_owned();
            writer.close().await.unwrap();

            let original = fs::read(&path).unwrap();
            let lines: Vec<&[u8]> = original.split(|&byte| byte == b'\n').filter(|line| !line.is_empty()).collect();
            assert_eq!(lines.len(), 2);
            let root_record: Value = serde_json::from_slice(lines[0]).unwrap();
            let child_record: Value = serde_json::from_slice(lines[1]).unwrap();

            for (field, value, expected) in [
                ("id", root_record["id"].clone(), LogError::DuplicateId),
                ("parent_id", Value::Null, LogError::MultipleOrMissingRoot),
                ("parent_id", json!(Uuid::now_v7()), LogError::DanglingParentId),
            ] {
                let mut child = child_record.clone();
                child[field] = value;
                let mut corrupted = lines[0].to_vec();
                corrupted.push(b'\n');
                corrupted.extend_from_slice(&serde_json::to_vec(&child).unwrap());
                corrupted.push(b'\n');
                fs::write(&path, &corrupted).unwrap();
                assert_eq!(read_session_log(&path), Err(expected));
                assert_eq!(fs::read(&path).unwrap(), corrupted);
            }

            let mut corrupted = lines[0].to_vec();
            corrupted.extend_from_slice(b"\nnot json\n");
            corrupted.extend_from_slice(lines[1]);
            corrupted.push(b'\n');
            fs::write(&path, &corrupted).unwrap();
            assert_eq!(read_session_log(&path), Err(LogError::CorruptLine));
            assert_eq!(fs::read(&path).unwrap(), corrupted);
        });
    }

    #[test]
    fn redaction_marker_and_structural_field_secrets_fail_closed() {
        for (secret, data) in [
            ("REDACTED", json!({"value": "REDACTED"})),
            ("session_start", json!({})),
            ("}\n", json!({})),
        ] {
            let dir = TestDir::new();
            runtime().block_on(async {
                let writer = SessionLogWriter::open(&dir.0, secret).await.unwrap();
                let path = writer.path().to_owned();
                let before = fs::read(&path).unwrap();
                assert_eq!(writer.append(None, 0, "rlm", "session_start", data).await, Err(LogError::InvalidRecord));
                assert_eq!(writer.append(None, 0, "rlm", "session_start", json!({})).await, Err(LogError::InvalidRecord));
                assert_eq!(writer.close().await, Err(LogError::InvalidRecord));
                let after = fs::read(&path).unwrap();
                assert_eq!(after, before);
                assert!(!after.windows(secret.len()).any(|window| window == secret.as_bytes()));
            });
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn actual_write_error_stops_later_writes_and_side_effects() {
    use serde_json::json;
    use std::{future::{poll_fn, Future}, task::Poll};
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(old: i32, new: i32) -> i32;
        fn close(fd: i32) -> i32;
    }
    let dir = TestDir::new();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        let writer = SessionLogWriter::open(&dir.0, "secret").await.unwrap();
        let root = writer.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
        let path = writer.path().to_owned();
        let before = fs::read(&path).unwrap();
        let fd = fs::read_dir("/proc/self/fd").unwrap().filter_map(Result::ok)
            .find_map(|entry| {
                (fs::read_link(entry.path()).ok().as_deref() == Some(path.as_path()))
                    .then(|| entry.file_name().to_string_lossy().parse::<i32>().ok()).flatten()
            }).expect("writer file descriptor");
        let full = fs::OpenOptions::new().write(true).open("/dev/full").unwrap();
        let saved = unsafe { dup(fd) };
        assert!(saved >= 0);
        assert_eq!(unsafe { dup2(full.as_raw_fd(), fd) }, fd);
        let mut failing = Box::pin(writer.append(Some(root), 0, "rlm", "pty_exec", json!({"index": 0})));
        let mut queued = Box::pin(writer.append(Some(root), 0, "rlm", "pty_exec", json!({"index": 1})));
        poll_fn(|cx| {
            assert!(matches!(failing.as_mut().poll(cx), Poll::Pending));
            assert!(matches!(queued.as_mut().poll(cx), Poll::Pending));
            Poll::Ready(())
        }).await;
        // Both requests have been polled before the single-threaded worker can run.
        let failed = failing.await;
        assert_eq!(unsafe { dup2(saved, fd) }, fd);
        assert_eq!(unsafe { close(saved) }, 0);
        assert_eq!(failed, Err(LogError::Io));
        let side_effect = dir.0.join("side-effect");
        let queued_result = queued.await;
        if queued_result.is_ok() {
            fs::write(&side_effect, b"started").unwrap();
        }
        assert_eq!(queued_result, Err(LogError::Io));
        assert!(!side_effect.exists());
        assert_eq!(writer.sync_turn().await, Err(LogError::Io));
        assert_eq!(writer.close().await, Err(LogError::Io));
        assert_eq!(fs::read(path).unwrap(), before);
    });
}

#[cfg(any(unix, windows))]
#[test]
fn redaction_key_collision_fails_closed() {
    use serde_json::json;
    let dir = TestDir::new();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        let writer = SessionLogWriter::open(&dir.0, "secret").await.unwrap();
        let root = writer.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
        let before = fs::read(writer.path()).unwrap();
        let data = json!({"nested": {"secret": 1, "[REDACTED]": 2}});
        assert_eq!(writer.append(Some(root), 0, "rlm", "pty_exec", data).await, Err(LogError::InvalidRecord));
        assert_eq!(writer.append(Some(root), 0, "rlm", "pty_exec", json!({})).await, Err(LogError::InvalidRecord));
        assert_eq!(writer.sync_turn().await, Err(LogError::InvalidRecord));
        assert_eq!(writer.close().await, Err(LogError::InvalidRecord));
        assert_eq!(fs::read(writer.path()).unwrap(), before);
    });
}

#[cfg(any(unix, windows))]
#[test]
fn cancelled_acknowledgement_is_not_a_side_effect_authorization() {
    use serde_json::json;
    use std::{future::{poll_fn, Future}, task::Poll};
    let dir = TestDir::new();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        let writer = SessionLogWriter::open(&dir.0, "secret").await.unwrap();
        let root = writer.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
        let mut append = Box::pin(writer.append(Some(root), 0, "rlm", "pty_exec", json!({})));
        let side_effect_started = false;
        poll_fn(|cx| {
            assert!(matches!(append.as_mut().poll(cx), Poll::Pending));
            Poll::Ready(())
        }).await;
        drop(append);
        // Cancellation is not an acknowledgement; do not start the side effect.
        // On this single-threaded runtime the queued command has not started yet.
        writer.sync_turn().await.unwrap();
        assert!(!side_effect_started);
        assert_eq!(ferrocore_driver::read_session_log(writer.path()).unwrap().len(), 1);
        writer.close().await.unwrap();
    });
}

#[cfg(any(unix, windows))]
#[test]
fn concurrent_producers_beyond_queue_capacity_are_acknowledged() {
    use serde_json::json;
    let dir = TestDir::new();
    let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
    runtime.block_on(async {
        let writer = SessionLogWriter::open(&dir.0, "secret").await.unwrap();
        let root = writer.append(None, 0, "rlm", "session_start", json!({})).await.unwrap();
        let mut tasks = Vec::new();
        for index in 0..160 {
            let writer = writer.clone();
            tasks.push(tokio::spawn(async move {
                writer.append(Some(root), 0, "rlm", "pty_exec", json!({"index": index})).await.unwrap()
            }));
        }
        let mut ids = std::collections::HashSet::new();
        for task in tasks { assert!(ids.insert(task.await.unwrap())); }
        writer.sync_turn().await.unwrap();
        let records = ferrocore_driver::read_session_log(writer.path()).unwrap();
        assert_eq!(records.len(), 161);
        assert!(records.iter().skip(1).all(|record| record.parent_id == Some(root) && ids.contains(&record.id)));
        writer.close().await.unwrap();
    });
}

#[cfg(windows)]
#[test]
fn windows_log_objects_inherit_private_acls() {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    type Handle = *mut c_void;
    #[repr(C)]
    struct AclSizeInformation { ace_count: u32, bytes_in_use: u32, bytes_free: u32 }
    #[repr(C)]
    struct AceHeader { kind: u8, flags: u8, size: u16 }
    #[repr(C)]
    struct AccessAllowedAce { header: AceHeader, mask: u32, sid_start: u32 }
    #[link(name = "advapi32")]
    extern "system" {
        fn GetNamedSecurityInfoW(name: *const u16, object_type: u32, info: u32, owner: *mut Handle, group: *mut Handle, dacl: *mut Handle, sacl: *mut Handle, descriptor: *mut Handle) -> u32;
        fn GetAclInformation(acl: Handle, info: *mut c_void, length: u32, class: u32) -> i32;
        fn GetAce(acl: Handle, index: u32, ace: *mut Handle) -> i32;
        fn EqualSid(first: Handle, second: Handle) -> i32;
        fn ConvertStringSidToSidW(text: *const u16, sid: *mut Handle) -> i32;
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(token: Handle, class: u32, info: *mut c_void, length: u32, needed: *mut u32) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: Handle) -> Handle;
    }
    unsafe fn check(path: &std::path::Path) {
        let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut descriptor = ptr::null_mut();
        let mut dacl = ptr::null_mut();
        assert_eq!(GetNamedSecurityInfoW(name.as_ptr(), 1, 4, ptr::null_mut(), ptr::null_mut(), &mut dacl, ptr::null_mut(), &mut descriptor), 0);
        assert!(!dacl.is_null());
        let mut token = ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), 8, &mut token), 0);
        let mut needed = 0;
        GetTokenInformation(token, 1, ptr::null_mut(), 0, &mut needed);
        let mut token_data = vec![0usize; (needed as usize + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()];
        assert_ne!(GetTokenInformation(token, 1, token_data.as_mut_ptr().cast(), needed, &mut needed), 0);
        let user = *(token_data.as_ptr() as *const Handle);
        let mut system = ptr::null_mut();
        let mut admins = ptr::null_mut();
        let system_text: Vec<u16> = "S-1-5-18\0".encode_utf16().collect();
        let admins_text: Vec<u16> = "S-1-5-32-544\0".encode_utf16().collect();
        assert_ne!(ConvertStringSidToSidW(system_text.as_ptr(), &mut system), 0);
        assert_ne!(ConvertStringSidToSidW(admins_text.as_ptr(), &mut admins), 0);
        let mut info = AclSizeInformation { ace_count: 0, bytes_in_use: 0, bytes_free: 0 };
        assert_ne!(GetAclInformation(dacl, (&mut info as *mut AclSizeInformation).cast(), std::mem::size_of::<AclSizeInformation>() as u32, 2), 0);
        assert!(info.ace_count > 0);
        let mut inherited_system = false;
        for index in 0..info.ace_count {
            let mut ace = ptr::null_mut();
            assert_ne!(GetAce(dacl, index, &mut ace), 0);
            let header = &*(ace as *const AceHeader);
            if header.kind != 0 {
                continue;
            }
            let ace = &*(ace as *const AccessAllowedAce);
            let sid = (&ace.sid_start as *const u32).cast_mut().cast();
            let is_user = EqualSid(sid, user) != 0;
            let is_system = EqualSid(sid, system) != 0;
            let is_admins = EqualSid(sid, admins) != 0;
            assert!(is_user || is_system || is_admins, "unexpected allowed principal");
            inherited_system |= is_system && ace.header.flags & 0x10 != 0;
        }
        assert!(inherited_system, "expected inherited SYSTEM ACE");
        LocalFree(system); LocalFree(admins); LocalFree(descriptor); CloseHandle(token);
    }
    for sid in ["S-1-5-18", "S-1-1-0"] {
        let dir = TestDir::new();
        let status = std::process::Command::new("icacls")
            .arg(&dir.0)
            .args(["/grant", &format!("*{sid}:(OI)(CI)F")])
            .status().unwrap();
        assert!(status.success(), "could not grant inheritable access to {sid}");
        let inherited_dir = dir.0.join("inherited");
        fs::create_dir(&inherited_dir).unwrap();
        unsafe {
            let name: Vec<u16> = inherited_dir.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut descriptor = ptr::null_mut();
            let mut dacl = ptr::null_mut();
            assert_eq!(GetNamedSecurityInfoW(name.as_ptr(), 1, 4, ptr::null_mut(), ptr::null_mut(), &mut dacl, ptr::null_mut(), &mut descriptor), 0);
            assert!(!dacl.is_null());
            let mut expected_sid = ptr::null_mut();
            let sid_text: Vec<u16> = sid.encode_utf16().chain(Some(0)).collect();
            assert_ne!(ConvertStringSidToSidW(sid_text.as_ptr(), &mut expected_sid), 0);
            let mut info = AclSizeInformation { ace_count: 0, bytes_in_use: 0, bytes_free: 0 };
            assert_ne!(GetAclInformation(dacl, (&mut info as *mut AclSizeInformation).cast(), std::mem::size_of::<AclSizeInformation>() as u32, 2), 0);
            let mut inherited = false;
            for index in 0..info.ace_count {
                let mut ace = ptr::null_mut();
                assert_ne!(GetAce(dacl, index, &mut ace), 0);
                let header = &*(ace as *const AceHeader);
                if header.kind == 0 && header.flags & 0x10 != 0 {
                    let allowed = &*(ace as *const AccessAllowedAce);
                    let ace_sid = (&allowed.sid_start as *const u32).cast_mut().cast();
                    inherited |= EqualSid(ace_sid, expected_sid) != 0;
                }
            }
            assert!(inherited, "expected inherited ACE for {sid}");
            LocalFree(expected_sid);
            LocalFree(descriptor);
        }
        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            if sid == "S-1-1-0" {
                assert!(matches!(SessionLogWriter::open(&inherited_dir, "secret").await, Err(LogError::InsecurePermissions)));
                for entry in fs::read_dir(&inherited_dir).unwrap() {
                    let entry = entry.unwrap();
                    assert!(entry.file_type().unwrap().is_dir(), "unexpected file after rejected open");
                    assert_eq!(fs::read_dir(entry.path()).unwrap().count(), 0, "rejected session directory must be empty");
                }
            } else {
                let writer = SessionLogWriter::open(&inherited_dir, "secret").await.unwrap();
                unsafe { check(writer.path().parent().unwrap()); check(writer.path()); }
                writer.close().await.unwrap();
            }
        });
    }
}
