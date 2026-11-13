#![cfg(any(target_os = "linux", windows))]

use std::{fs, path::PathBuf, sync::atomic::{AtomicU64, Ordering}};

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!("ferrocore-output-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("cannot create test directory: {error}"),
            }
        }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

use ferrocore_runtime_pty::output::{OutputCapture, OutputView};

fn capture_with_chunk_size(dir: &TestDir, bytes: &[u8], chunk_size: usize) -> OutputView {
    let mut capture = OutputCapture::new(&dir.0, 16_384).unwrap();
    for chunk in bytes.chunks(chunk_size) { capture.write_chunk(chunk).unwrap(); }
    let view = capture.finish().unwrap();
    assert_eq!(fs::read(dir.0.join("last_output.log")).unwrap(), bytes);
    view
}

fn capture(dir: &TestDir, bytes: &[u8]) -> OutputView {
    capture_with_chunk_size(dir, bytes, 137)
}

#[test]
fn byte_threshold_is_strict_and_raw_output_is_published() {
    let dir = TestDir::new();
    for (size, pruned) in [(10_240, false), (10_241, true)] {
        let input = vec![b'x'; size];
        let view = capture(&dir, &input);
        assert_eq!(view.raw_bytes, size as u64);
        assert_eq!(view.lines, 1);
        assert_eq!(view.pruned, pruned);
        assert!(!view.invalid_utf8);
        assert!(view.text.len() <= 16_384);
        if !pruned { assert_eq!(view.text.as_bytes(), input.as_slice()); }
    }
}

#[test]
fn line_threshold_crlf_final_fragment_and_empty() {
    let dir = TestDir::new();
    let empty = capture(&dir, b"");
    assert_eq!((empty.raw_bytes, empty.lines, empty.pruned, empty.invalid_utf8), (0, 0, false, false));
    assert!(empty.text.is_empty());

    let single = capture(&dir, b"only");
    assert_eq!((single.lines, single.pruned), (1, false));
    let lone_lf = capture(&dir, b"\n");
    assert_eq!((lone_lf.lines, lone_lf.pruned), (1, false));

    // Split across \r and \n boundaries with chunk size 3.
    let crlf_terminated = capture_with_chunk_size(&dir, b"first\r\nsecond\r\n", 3);
    assert_eq!((crlf_terminated.lines, crlf_terminated.pruned), (2, false));
    let crlf_fragment = capture_with_chunk_size(&dir, b"first\r\nsecond\r\nlast", 3);
    assert_eq!((crlf_fragment.lines, crlf_fragment.pruned), (3, false));

    for (count, terminated) in [(150, true), (150, false), (151, true), (151, false)] {
        let mut input = "line\r\n".repeat(count);
        if !terminated { input.truncate(input.len() - 2); }
        let view = capture_with_chunk_size(&dir, input.as_bytes(), 5);
        assert_eq!(view.lines, count as u64);
        assert_eq!(view.pruned, count > 150);
    }
}

#[test]
fn overlapping_first_and_last_lines_are_not_duplicated() {
    let dir = TestDir::new();
    let within_view_limit: String = (0..80).map(|i| format!("line-{i:03}:{}\n", "x".repeat(130))).collect();
    let view = capture(&dir, within_view_limit.as_bytes());
    assert!(view.pruned);
    assert_eq!(view.lines, 80);
    for i in 0..80 {
        let marker = format!("line-{i:03}:");
        assert_eq!(view.text.matches(&marker).count(), 1, "{marker}");
    }

    let exceeds_view_limit: String = (0..80).map(|i| format!("row-{i:03}:{}\n", "y".repeat(300))).collect();
    let bounded = capture(&dir, exceeds_view_limit.as_bytes());
    assert!(bounded.pruned);
    assert!(bounded.text.len() <= 16_384);
    for i in 0..80 {
        let marker = format!("row-{i:03}:");
        assert!(bounded.text.matches(&marker).count() <= 1, "duplicated {marker}");
    }
}

#[test]
fn first_and_last_fifty_lines_exclude_the_middle() {
    let dir = TestDir::new();
    let input: String = (0..151).map(|i| format!("line-{i:03}\n")).collect();
    let view = capture(&dir, input.as_bytes());
    assert!(view.pruned);
    assert_eq!(view.lines, 151);
    for i in 0..151 {
        let marker = format!("line-{i:03}\n");
        assert_eq!(view.text.matches(&marker).count(), usize::from(i < 50 || i >= 101), "{marker}");
    }
}

#[test]
fn utf8_boundaries_and_invalid_bytes_preserve_raw_log() {
    let dir = TestDir::new();
    let input = format!(
        "{}{}{}{}{}",
        "a".repeat(8_000),
        "🦀".repeat(500),
        "b".repeat(10_000),
        "🦀".repeat(500),
        "c".repeat(8_000),
    );
    let view = capture_with_chunk_size(&dir, input.as_bytes(), 7);
    assert!(view.pruned);
    assert!(!view.invalid_utf8);
    assert!(view.text.len() <= 16_384);
    assert!(!view.text.contains('\u{fffd}'));

    let short_invalid = capture(&dir, b"before\xff\xfeafter");
    assert!(!short_invalid.pruned);
    assert!(short_invalid.invalid_utf8);
    assert!(short_invalid.text.contains('\u{fffd}'));
    assert!(short_invalid.text.contains("UTF-8"));

    let mut long_invalid = Vec::new();
    long_invalid.extend_from_slice(b"head\xff");
    long_invalid.extend_from_slice(&vec![b'm'; 20_000]);
    long_invalid.extend_from_slice(b"\xfe\x80tail");
    let long_view = capture(&dir, &long_invalid);
    assert!(long_view.pruned);
    assert!(long_view.invalid_utf8);
    assert!(long_view.text.len() <= 16_384);
    assert!(long_view.text.contains('\u{fffd}'));
    assert!(long_view.text.contains("UTF-8"));
}

#[test]
fn large_output_is_written_before_finish() {
    let dir = TestDir::new();
    let mut capture = OutputCapture::new(&dir.0, 16_384).unwrap();
    let chunk = vec![b'z'; 65_536];
    for _ in 0..64 { capture.write_chunk(&chunk).unwrap(); }
    let files: Vec<_> = fs::read_dir(&dir.0).unwrap().map(|entry| entry.unwrap().path()).collect();
    assert_eq!(files.len(), 1);
    assert!(fs::metadata(&files[0]).unwrap().len() >= 3 * 1024 * 1024);
    assert!(!dir.0.join("last_output.log").exists());
    for _ in 0..64 { capture.write_chunk(&chunk).unwrap(); }
    assert!(fs::metadata(&files[0]).unwrap().len() >= 7 * 1024 * 1024);
    let view = capture.finish().unwrap();
    assert_eq!(view.raw_bytes, 8 * 1024 * 1024);
    assert!(view.text.len() <= 16_384);
    assert_eq!(fs::metadata(dir.0.join("last_output.log")).unwrap().len(), view.raw_bytes);
}

#[test]
fn publish_io_error_does_not_succeed_when_destination_is_directory() {
    let dir = TestDir::new();
    let destination = dir.0.join("last_output.log");
    fs::create_dir_all(destination.join("nested")).unwrap();
    let mut output = OutputCapture::new(&dir.0, 16_384).unwrap();
    output.write_chunk(b"new output").unwrap();
    assert!(output.finish().is_err());
    assert!(destination.is_dir());
}

#[cfg(target_os = "linux")]
#[test]
fn disk_write_error_keeps_previous_successful_file() {
    const CHILD: &str = "FERROCORE_OUTPUT_FSIZE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let test_name = std::thread::current().name().expect("test thread name").to_owned();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name, "--nocapture"])
            .env(CHILD, "1").status().unwrap();
        assert!(status.success(), "isolated disk error test failed: {status}");
        return;
    }
    let dir = TestDir::new();
    capture(&dir, b"previous successful output");
    let mut output = OutputCapture::new(&dir.0, 16_384).unwrap();
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
        let limit = libc::rlimit { rlim_cur: 4096, rlim_max: 4096 };
        assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
    }
    assert!(output.write_chunk(&vec![b'x'; 131_072]).is_err());
    assert!(output.write_chunk(b"after error").is_err());
    assert!(output.write_chunk(b"").is_err());
    assert!(output.finish().is_err());
    assert_eq!(fs::read(dir.0.join("last_output.log")).unwrap(), b"previous successful output");
}

#[cfg(windows)]
fn open_windows_reader(path: &std::path::Path, share_delete: bool) -> fs::File {
    use std::{ffi::c_void, os::windows::{ffi::OsStrExt, io::FromRawHandle}, ptr};
    #[link(name = "kernel32")]
    extern "system" {
        fn CreateFileW(path: *const u16, access: u32, share: u32, attributes: *const c_void, creation: u32, flags: u32, template: *mut c_void) -> *mut c_void;
    }
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_SHARE_READ: u32 = 1;
    const FILE_SHARE_WRITE: u32 = 2;
    const FILE_SHARE_DELETE: u32 = 4;
    const OPEN_EXISTING: u32 = 3;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE | if share_delete { FILE_SHARE_DELETE } else { 0 };
    let handle = unsafe { CreateFileW(wide.as_ptr(), GENERIC_READ, share, ptr::null(), OPEN_EXISTING, 0, ptr::null_mut()) };
    assert_ne!(handle as isize, -1, "CreateFileW failed: {}", std::io::Error::last_os_error());
    unsafe { fs::File::from_raw_handle(handle) }
}

#[cfg(windows)]
fn observe_windows_rename(share_delete: bool) {
    use std::io::Read;
    let dir = TestDir::new();
    let destination = dir.0.join("last_output.log");
    let replacement = dir.0.join("replacement.tmp");
    fs::write(&destination, b"old output").unwrap();
    fs::write(&replacement, b"new output").unwrap();
    let mut reader = open_windows_reader(&destination, share_delete);
    let result = fs::rename(&replacement, &destination);
    let mut opened_contents = Vec::new();
    reader.read_to_end(&mut opened_contents).unwrap();
    let destination_contents = fs::read(&destination);
    let replacement_contents = fs::read(&replacement);
    drop(reader);
    eprintln!("share_delete={share_delete}, rename={result:?}, existing_handle={opened_contents:?}, destination={destination_contents:?}, replacement={replacement_contents:?}");
    assert_eq!(opened_contents, b"old output");
    if share_delete {
        result.expect("rename with FILE_SHARE_DELETE reader should succeed");
        assert_eq!(destination_contents.unwrap(), b"new output");
        assert_eq!(replacement_contents.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    } else {
        let error = result.expect_err("rename without FILE_SHARE_DELETE reader should fail");
        eprintln!("rename error without FILE_SHARE_DELETE: {error:?}");
        assert_eq!(destination_contents.unwrap(), b"old output");
        assert_eq!(replacement_contents.unwrap(), b"new output");
    }
}

#[cfg(windows)]
fn observe_windows_output_capture(share_delete: bool) {
    use std::io::Read;
    let dir = TestDir::new();
    capture(&dir, b"old output");
    let destination = dir.0.join("last_output.log");
    let mut reader = open_windows_reader(&destination, share_delete);
    let mut next = OutputCapture::new(&dir.0, 16_384).unwrap();
    next.write_chunk(b"new output").unwrap();
    let result = next.finish();
    let mut opened_contents = Vec::new();
    reader.read_to_end(&mut opened_contents).unwrap();
    let destination_contents = fs::read(&destination);
    drop(reader);
    assert_eq!(opened_contents, b"old output");
    if share_delete {
        let view = result.expect("OutputCapture finish with FILE_SHARE_DELETE reader should succeed");
        assert_eq!(view.raw_bytes, 10);
        assert_eq!(destination_contents.unwrap(), b"new output");
    } else {
        assert!(result.is_err(), "OutputCapture finish without FILE_SHARE_DELETE reader should fail: {result:?}");
        assert_eq!(destination_contents.unwrap(), b"old output");
    }
}

#[cfg(windows)]
#[test]
fn windows_rename_with_delete_shared_reader() { observe_windows_rename(true); }

#[cfg(windows)]
#[test]
fn windows_rename_with_delete_denied_reader() { observe_windows_rename(false); }

#[cfg(windows)]
#[test]
fn windows_output_capture_with_delete_shared_reader() { observe_windows_output_capture(true); }

#[cfg(windows)]
#[test]
fn windows_output_capture_with_delete_denied_reader() { observe_windows_output_capture(false); }

#[cfg(windows)]
#[test]
fn write_chunk_io_error_poisons_capture_and_finish_does_not_publish() {
    use std::{ffi::c_void, os::windows::io::AsRawHandle, ptr};
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut c_void,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LockFileEx(file: *mut c_void, flags: u32, reserved: u32, len_low: u32, len_high: u32, overlapped: *mut Overlapped) -> i32;
        fn UnlockFileEx(file: *mut c_void, reserved: u32, len_low: u32, len_high: u32, overlapped: *mut Overlapped) -> i32;
    }
    const LOCKFILE_FAIL_IMMEDIATELY: u32 = 1;
    const LOCKFILE_EXCLUSIVE_LOCK: u32 = 2;

    let dir = TestDir::new();
    capture(&dir, b"previous successful output");
    let mut output = OutputCapture::new(&dir.0, 16_384).unwrap();
    let tmp_path = fs::read_dir(&dir.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.file_name().is_some_and(|name| name.to_string_lossy().starts_with(".last_output.")))
        .expect("temporary output file should exist");
    let locker = fs::OpenOptions::new().read(true).write(true).open(&tmp_path).unwrap();
    let mut overlapped = Overlapped { internal: 0, internal_high: 0, offset: 0, offset_high: 0, event: ptr::null_mut() };
    assert_ne!(unsafe { LockFileEx(locker.as_raw_handle(), LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK, 0, u32::MAX, 0, &mut overlapped) }, 0);

    assert!(output.write_chunk(&vec![b'x'; 16_384]).is_err());
    assert_ne!(unsafe { UnlockFileEx(locker.as_raw_handle(), 0, u32::MAX, 0, &mut overlapped) }, 0);
    drop(locker);

    assert!(output.write_chunk(b"still poisoned").is_err());
    assert!(output.write_chunk(b"").is_err());
    assert!(output.finish().is_err());
    assert!(!tmp_path.exists());
    assert_eq!(fs::read(dir.0.join("last_output.log")).unwrap(), b"previous successful output");
}

#[test]
fn incremental_utf8_across_chunks_and_omitted_middle() {
    let dir = TestDir::new();
    let valid_multibyte = "a©€💩b";
    let valid_view = capture_with_chunk_size(&dir, valid_multibyte.as_bytes(), 1);
    assert!(!valid_view.invalid_utf8);
    assert_eq!(valid_view.text, valid_multibyte);

    let split_c2_a = capture_with_chunk_size(&dir, b"\xc2A", 1);
    assert!(split_c2_a.invalid_utf8);
    assert!(split_c2_a.text.contains('\u{fffd}'));
    assert!(split_c2_a.text.contains('A'));

    let split_e2_82_a = capture_with_chunk_size(&dir, b"\xe2\x82A", 1);
    assert!(split_e2_82_a.invalid_utf8);
    assert!(split_e2_82_a.text.contains('\u{fffd}'));

    for trailing in [b"ok\xc2".as_slice(), b"ok\xf0\x9f".as_slice()] {
        let view = capture_with_chunk_size(&dir, trailing, 1);
        assert!(view.invalid_utf8);
        assert!(view.text.contains('\u{fffd}'));
    }

    let mut middle_invalid_unsplit = vec![b'a'; 20_000];
    middle_invalid_unsplit.extend_from_slice(b"\xc2A");
    middle_invalid_unsplit.extend_from_slice(&vec![b'b'; 20_000]);
    let middle_view = capture_with_chunk_size(&dir, &middle_invalid_unsplit, 13);
    assert!(middle_view.pruned);
    assert!(middle_view.invalid_utf8);
    assert!(middle_view.text.contains("UTF-8"));

    let mut middle_invalid_lines = "valid-head-line\n".repeat(80).into_bytes();
    middle_invalid_lines.extend_from_slice(b"bad-middle-\xc2A\n");
    middle_invalid_lines.extend_from_slice(&"valid-tail-line\n".repeat(80).into_bytes());
    let middle_lines_view = capture_with_chunk_size(&dir, &middle_invalid_lines, 7);
    assert!(middle_lines_view.pruned);
    assert!(middle_lines_view.invalid_utf8);
    assert!(middle_lines_view.text.contains("UTF-8"));
}

#[test]
fn cut_slice_detects_invalid_0xc2_followed_by_ascii_a() {
    let dir = TestDir::new();

    // Place 0xC2 right at the `view_limit + 4` head cut boundary followed by 'A'.
    let mut unsplit = vec![b'a'; 203];
    unsplit.extend_from_slice(b"\xc2A");
    unsplit.extend_from_slice(&vec![b'b'; 200]);
    let mut cap = OutputCapture::new(&dir.0, 200).unwrap();
    for chunk in unsplit.chunks(204) {
        cap.write_chunk(chunk).unwrap();
    }
    let unsplit_view = cap.finish().unwrap();
    assert!(unsplit_view.invalid_utf8);
    assert!(unsplit_view.text.contains("UTF-8"));
    assert!(unsplit_view.text.len() <= 200);

    // Same boundary inside a >150-line pruned view where line 0 exceeds `head_cap`.
    let mut pruned = vec![b'a'; 203];
    pruned.extend_from_slice(b"\xc2A");
    pruned.extend_from_slice(&vec![b'a'; 100]);
    pruned.push(b'\n');
    pruned.extend_from_slice(&"line\n".repeat(150).into_bytes());
    let mut cap = OutputCapture::new(&dir.0, 200).unwrap();
    for chunk in pruned.chunks(204) {
        cap.write_chunk(chunk).unwrap();
    }
    let pruned_view = cap.finish().unwrap();
    assert!(pruned_view.pruned);
    assert!(pruned_view.invalid_utf8);
    assert!(pruned_view.text.contains("UTF-8"));
    assert!(pruned_view.text.len() <= 200);
}

#[test]
fn insufficient_view_limit_for_notice_returns_error_without_publishing() {
    let dir = TestDir::new();

    // Short valid output without a notice succeeds even with a small view_limit.
    let mut ok = OutputCapture::new(&dir.0, 20).unwrap();
    ok.write_chunk(b"short").unwrap();
    let ok_view = ok.finish().unwrap();
    assert_eq!(ok_view.text, "short");
    assert_eq!(fs::read(dir.0.join("last_output.log")).unwrap(), b"short");

    capture(&dir, b"previous output");
    for payload in [
        b"this output exceeds twenty bytes".as_slice(),
        b"a\xff".as_slice(),
        &"l\n".repeat(151).into_bytes(),
    ] {
        let mut cap = OutputCapture::new(&dir.0, 20).unwrap();
        cap.write_chunk(payload).unwrap();
        let error = cap.finish().expect_err("view_limit too small for required notice must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert_eq!(fs::read(dir.0.join("last_output.log")).unwrap(), b"previous output");
    }
}
