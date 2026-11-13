use std::io;
use std::path::{Path, PathBuf};

/// A scratch directory is retained unless its owner explicitly cleans it up after
/// confirming that every process which could use it has stopped.
#[derive(Debug)]
pub struct Scratch {
    path: PathBuf,
    fallback: bool,
}

impl Scratch {
    pub fn new() -> io::Result<Self> {
        create()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// True when Linux shared memory was unavailable and /tmp was used instead.
    pub fn fallback_used(&self) -> bool {
        self.fallback
    }

    /// Call only after the shell and any other users are known to have stopped.
    pub fn cleanup(self) -> io::Result<()> {
        std::fs::remove_dir_all(&self.path)
    }
}

fn random_name() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    random_bytes(&mut bytes)?;
    let mut name = String::from("ferrocore_");
    use std::fmt::Write;
    for byte in bytes {
        write!(&mut name, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(name)
}

#[cfg(target_os = "linux")]
fn random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(target_os = "linux")]
fn create_at(base: &Path, fallback: bool) -> io::Result<Scratch> {
    use std::os::unix::fs::DirBuilderExt;
    for _ in 0..128 {
        let path = base.join(random_name()?);
        match std::fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {
                use std::os::unix::fs::PermissionsExt;
                if let Err(error) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)) {
                    let _ = std::fs::remove_dir(&path);
                    return Err(error);
                }
                if !fallback {
                    use std::io::Write;
                    let probe = path.join(".capacity-probe");
                    let result = std::fs::OpenOptions::new().write(true).create_new(true).open(&probe)
                        .and_then(|mut file| file.write_all(&[0]));
                    let _ = std::fs::remove_file(&probe);
                    if let Err(error) = result {
                        let _ = std::fs::remove_dir(&path);
                        return Err(error);
                    }
                }
                return Ok(Scratch { path, fallback });
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "scratch name collision limit reached"))
}

#[cfg(target_os = "linux")]
fn create() -> io::Result<Scratch> {
    match create_at(Path::new("/dev/shm"), false) {
        Ok(scratch) => Ok(scratch),
        Err(error) if matches!(error.raw_os_error(), Some(1 | 2 | 13 | 28 | 30)) => {
            create_at(Path::new("/tmp"), true)
        }
        Err(error) => Err(error),
    }
}

#[cfg(windows)]
fn random_bytes(bytes: &mut [u8]) -> io::Result<()> {
    #[link(name = "bcrypt")]
    extern "system" {
        fn BCryptGenRandom(algorithm: *mut std::ffi::c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
    }
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), bytes.len() as u32, 2) };
    if status < 0 {
        Err(io::Error::new(io::ErrorKind::Other, "system random generator failed"))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn create() -> io::Result<Scratch> {
    windows::create(&std::env::temp_dir())
}

#[cfg(not(any(target_os = "linux", windows)))]
fn random_bytes(_: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "scratch is supported only on Linux and Windows"))
}

#[cfg(not(any(target_os = "linux", windows)))]
fn create() -> io::Result<Scratch> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "scratch is supported only on Linux and Windows"))
}

#[cfg(windows)]
mod windows {
    use super::{random_name, Scratch};
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
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
        fn CreateFileW(path: *const u16, access: u32, share: u32, attributes: *const c_void, creation: u32, flags: u32, template: Handle) -> Handle;
        fn GetFileInformationByHandle(handle: Handle, info: *mut FileInfo) -> i32;
    }
    struct OwnedHandle(Handle);
    impl Drop for OwnedHandle { fn drop(&mut self) { unsafe { CloseHandle(self.0); } } }
    struct Local(*mut c_void);
    impl Drop for Local { fn drop(&mut self) { unsafe { if !self.0.is_null() { LocalFree(self.0); } } } }

    fn insecure() -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, "scratch directory security could not be verified")
    }

    unsafe fn verify(handle: Handle) -> io::Result<()> {
        let mut info: FileInfo = std::mem::zeroed();
        if GetFileInformationByHandle(handle, &mut info) == 0 || info.attributes & 0x400 != 0 || info.attributes & 0x10 == 0 { return Err(insecure()); }
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), 8, &mut token) == 0 { return Err(insecure()); }
        let token = OwnedHandle(token);
        let mut needed = 0;
        GetTokenInformation(token.0, 1, null_mut(), 0, &mut needed);
        if needed == 0 { return Err(insecure()); }
        let mut buffer = vec![0usize; (needed as usize + std::mem::size_of::<usize>() - 1) / std::mem::size_of::<usize>()];
        if GetTokenInformation(token.0, 1, buffer.as_mut_ptr().cast(), needed, &mut needed) == 0 { return Err(insecure()); }
        let user = (*(buffer.as_ptr() as *const TokenUser)).sid;
        if user.is_null() { return Err(insecure()); }
        let (mut system, mut admins) = ([0u32; 17], [0u32; 17]);
        let (mut system_len, mut admins_len) = (68, 68);
        if CreateWellKnownSid(22, null_mut(), system.as_mut_ptr().cast(), &mut system_len) == 0 || CreateWellKnownSid(26, null_mut(), admins.as_mut_ptr().cast(), &mut admins_len) == 0 { return Err(insecure()); }
        let (system, admins) = (system.as_mut_ptr() as Sid, admins.as_mut_ptr() as Sid);
        let (mut owner, mut acl, mut descriptor) = (null_mut(), null_mut(), null_mut());
        let status = GetSecurityInfo(handle, 1, 5, &mut owner, null_mut(), &mut acl, null_mut(), &mut descriptor);
        let _descriptor = Local(descriptor);
        if status != 0 || owner.is_null() || acl.is_null() || descriptor.is_null() || (EqualSid(owner, user) == 0 && EqualSid(owner, system) == 0 && EqualSid(owner, admins) == 0) { return Err(insecure()); }
        let mut size = AclSize { count: 0, used: 0, free: 0 };
        if GetAclInformation(acl, (&mut size as *mut AclSize).cast(), std::mem::size_of::<AclSize>() as u32, 2) == 0 || size.count == 0 { return Err(insecure()); }
        for index in 0..size.count {
            let mut ace = null_mut();
            if GetAce(acl, index, &mut ace) == 0 || ace.is_null() { return Err(insecure()); }
            let header = &*(ace as *const AceHeader);
            if header.kind != 0 || header.flags & !0x1f != 0 || header.size < 16 { return Err(insecure()); }
            let sid = &(*(ace as *const AllowAce)).sid_start as *const u32 as Sid;
            let bytes = sid as *const u8;
            let len = 8usize + (*bytes.add(1) as usize) * 4;
            if *bytes != 1 || len > header.size as usize - 8 || (EqualSid(sid, user) == 0 && EqualSid(sid, system) == 0 && EqualSid(sid, admins) == 0) { return Err(insecure()); }
        }
        Ok(())
    }

    pub(super) fn create(base: &Path) -> io::Result<Scratch> {
        for _ in 0..128 {
            let path = base.join(random_name()?);
            match std::fs::create_dir(&path) {
                Ok(()) => {},
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let handle = unsafe { CreateFileW(wide.as_ptr(), 0x00020000, 3, null(), 3, 0x02200000, null_mut()) };
            if handle as isize == -1 {
                let _ = std::fs::remove_dir(&path);
                return Err(insecure());
            }
            let handle = OwnedHandle(handle);
            let result = unsafe { verify(handle.0) };
            drop(handle);
            if let Err(error) = result {
                let _ = std::fs::remove_dir(&path);
                return Err(error);
            }
            return Ok(Scratch { path, fallback: false });
        }
        Err(io::Error::new(io::ErrorKind::AlreadyExists, "scratch name collision limit reached"))
    }
}

#[cfg(all(test, any(target_os = "linux", windows)))]
mod cleanup_tests {
    use super::Scratch;
    use std::process::Command;

    #[cfg(windows)]
    fn isolate_windows_test() -> bool {
        use std::{ffi::c_void, ptr, sync::atomic::{AtomicU64, Ordering}};
        if std::env::var_os("FERROCORE_SCRATCH_TEST_CHILD").is_some() { return false; }
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
            let path = std::env::temp_dir().join(format!("ferrocore-scratch-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
            match std::fs::create_dir(&path) {
                Ok(()) => break path,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => panic!("cannot create isolated TEMP: {e}"),
            }
        };
        let status = Command::new("icacls").arg(&dir)
            .args(["/inheritance:r", "/grant:r"])
            .arg(format!("*{user}:(OI)(CI)F"))
            .args(["*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F"])
            .status().expect("icacls required for isolated TEMP");
        assert!(status.success(), "could not secure isolated TEMP");
        let test_name = std::thread::current().name().expect("test thread name").to_owned();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &test_name, "--nocapture"])
            .env("FERROCORE_SCRATCH_TEST_CHILD", "1").env("TEMP", &dir).env("TMP", &dir)
            .status().expect("cannot spawn isolated test");
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(status.success(), "isolated test {test_name} failed: {status}");
        true
    }

    #[cfg(not(windows))]
    fn isolate_windows_test() -> bool { false }

    #[test]
    fn cleanup_before_shell_start_removes_contents() {
        if isolate_windows_test() { return; }
        let scratch = Scratch::new().unwrap();
        let path = scratch.path().to_owned();
        std::fs::write(path.join("output"), b"before spawn").unwrap();

        scratch.cleanup().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn failed_pre_spawn_cleanup_reports_retained_path() {
        if isolate_windows_test() { return; }
        let scratch = Scratch::new().unwrap();
        let path = scratch.path().to_owned();
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, b"not a directory").unwrap();

        let error = crate::cleanup_before_spawn(scratch, "spawn failed".into());
        assert_eq!(error.retained_scratch.as_deref(), Some(path.as_path()));
        assert!(error.message.contains("cleanup failed"));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cleanup_after_known_process_stops_removes_contents() {
        if isolate_windows_test() { return; }
        let scratch = Scratch::new().unwrap();
        let path = scratch.path().to_owned();
        #[cfg(target_os = "linux")]
        let mut command = {
            let mut command = Command::new("sh");
            command.args(["-c", "printf stopped > \"$FC_SCRATCH/output\""]);
            command
        };
        #[cfg(windows)]
        let mut command = {
            let mut command = Command::new("pwsh.exe");
            command.args(["-NoLogo", "-NoProfile", "-Command", "[IO.File]::WriteAllText((Join-Path $env:FC_SCRATCH 'output'), 'stopped')"]);
            command
        };
        let status = command.env("FC_SCRATCH", &path).spawn().unwrap().wait().unwrap();
        assert!(status.success());
        assert!(path.join("output").is_file());

        scratch.cleanup().unwrap();
        assert!(!path.exists());
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn scratch_is_private_unique_and_retained_until_cleanup() {
        let first = create_at(Path::new("/tmp"), true).unwrap();
        let second = create_at(Path::new("/tmp"), true).unwrap();
        assert_ne!(first.path(), second.path());
        assert_eq!(std::fs::metadata(first.path()).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(first.fallback_used());
        let retained = first.path().to_owned();
        drop(first);
        assert!(retained.is_dir());
        std::fs::remove_dir_all(retained).unwrap();
        let cleaned = second.path().to_owned();
        second.cleanup().unwrap();
        assert!(!cleaned.exists());
    }
}
