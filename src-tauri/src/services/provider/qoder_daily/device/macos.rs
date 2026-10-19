//! Native macOS gate. No fallback on policy/API failure. START_SUSPENDED binds
//! the direct child's actual signed image before this parent resumes it. This
//! guards file replacement races, not a hostile same-UID process sending SIGCONT.
use super::{install, Dependency, Platform, MAX_OUTPUT};
use std::{
    ffi::{c_char, c_void, CString},
    fs::File,
    io::{Read, Write},
    os::fd::{AsRawFd, FromRawFd},
    process::{Command, Stdio},
    ptr,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

pub(super) struct Native;
impl Platform for Native {
    type Installation = install::Installation;
    type Child = Suspended;
    fn verify_installation(&self) -> Result<Self::Installation, Dependency> {
        let installation = install::Installation::open()?;
        native_command(
            "/usr/bin/codesign",
            &["--verify", "--deep", "--strict", install::ROOT],
            Duration::from_secs(20),
        )?;
        static_signature(install::ROOT, install::REQUIREMENT)?;
        static_signature(install::HELPER, install::HELPER_REQUIREMENT)?;
        execution_policy()?;
        installation.recheck()?;
        Ok(installation)
    }
    fn start_suspended(
        &self,
        installation: &Self::Installation,
    ) -> Result<Self::Child, Dependency> {
        installation.recheck()?;
        Suspended::start()
    }
    fn verify_running_image(
        &self,
        installation: &Self::Installation,
        child: &mut Self::Child,
    ) -> Result<(), Dependency> {
        child.require_live()?;
        installation.recheck()?;
        dynamic_signature(
            child.pid,
            child.deadline.saturating_duration_since(Instant::now()),
        )?;
        child.require_live()?;
        installation.recheck()?;
        child.remaining()
    }
    fn resume_and_read(
        &self,
        child: &mut Self::Child,
        input: &[u8],
    ) -> Result<Zeroizing<Vec<u8>>, Dependency> {
        child.remaining()?;
        child.require_live()?;
        if unsafe { libc::kill(child.pid, libc::SIGCONT) } != 0 {
            return Err(Dependency::OfficialDeviceFailed);
        }
        child
            .input
            .take()
            .ok_or(Dependency::OfficialDeviceFailed)?
            .write_all(input)
            .map_err(|_| Dependency::OfficialDeviceFailed)?;
        let mut output = Zeroizing::new(Vec::new());
        let mut discarded = 0usize;
        loop {
            child.remaining()?;
            drain(&mut child.output, &mut output, MAX_OUTPUT)?;
            let mut stderr = Zeroizing::new(Vec::new());
            drain(&mut child.error, &mut stderr, 65536)?;
            discarded += stderr.len();
            if discarded > 65536 {
                return Err(Dependency::OfficialDeviceFailed);
            }
            if let Some(status) = child.poll()? {
                if !libc::WIFEXITED(status) || libc::WEXITSTATUS(status) != 0 {
                    return Err(Dependency::OfficialDeviceFailed);
                }
                // Last pipe writes can occur between the initial drain and exit.
                drain(&mut child.output, &mut output, MAX_OUTPUT)?;
                let mut final_stderr = Zeroizing::new(Vec::new());
                drain(&mut child.error, &mut final_stderr, 65536)?;
                if discarded + final_stderr.len() > 65536 {
                    return Err(Dependency::OfficialDeviceFailed);
                }
            }
            if let Some(end) = output.iter().position(|b| *b == b'\n') {
                output.truncate(end + 1);
                return Ok(output);
            }
            if child.ownership != Ownership::Live {
                return Err(Dependency::OfficialDeviceFailed);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn failed<T>() -> Result<T, Dependency> {
    Err(Dependency::OfficialDeviceUnverified)
}
fn nonblocking(file: &File) -> Result<(), Dependency> {
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return failed();
    }
    Ok(())
}
fn drain(reader: &mut impl Read, output: &mut Vec<u8>, limit: usize) -> Result<(), Dependency> {
    let mut buffer = Zeroizing::new([0u8; 4096]);
    loop {
        match reader.read(&mut buffer[..]) {
            Ok(0) => return Ok(()),
            Ok(n) if output.len() + n <= limit => output.extend_from_slice(&buffer[..n]),
            Ok(_) => return Err(Dependency::OfficialDeviceFailed),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(Dependency::OfficialDeviceFailed),
        }
    }
}

// Public Apple interfaces only. A numeric codesign operand means a dynamic PID,
// not a path. Explicit requirements use the documented '=' literal prefix.
fn static_signature(path: &str, requirement: &str) -> Result<(), Dependency> {
    let requirement = format!("={requirement}");
    native_command(
        "/usr/bin/codesign",
        &["--verify", "--strict", "-R", &requirement, path],
        Duration::from_secs(20),
    )
    .map(|_| ())
}
fn dynamic_signature(pid: libc::pid_t, timeout: Duration) -> Result<(), Dependency> {
    if pid <= 0 || timeout.is_zero() {
        return failed();
    }
    let pid = pid.to_string();
    let requirement = format!("={}", install::HELPER_REQUIREMENT);
    native_command(
        "/usr/bin/codesign",
        &["--verify", "--strict", "-R", &requirement, &pid],
        timeout,
    )
    .map(|_| ())
}
fn native_command(tool: &str, args: &[&str], timeout: Duration) -> Result<Vec<u8>, Dependency> {
    if timeout.is_zero() {
        return failed();
    }
    let mut child = Command::new(tool)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| Dependency::OfficialDeviceUnverified)?;
    let result = (|| {
        let mut stdout = child
            .stdout
            .take()
            .ok_or(Dependency::OfficialDeviceUnverified)?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or(Dependency::OfficialDeviceUnverified)?;
        for fd in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
            {
                return failed();
            }
        }
        let end = Instant::now() + timeout;
        let mut bytes = Vec::new();
        let mut diagnostics = Zeroizing::new(Vec::new());
        loop {
            drain(&mut stdout, &mut bytes, 65536)?;
            drain(&mut stderr, &mut diagnostics, 65536)?;
            if let Some(status) = child
                .try_wait()
                .map_err(|_| Dependency::OfficialDeviceUnverified)?
            {
                if !status.success() {
                    return failed();
                }
                drain(&mut stdout, &mut bytes, 65536)?;
                return Ok(bytes);
            }
            if Instant::now() >= end {
                return failed();
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
// A raw assessment must explicitly allow execution without an override. This
// is not Enforce SPI: privileged concurrent global-policy changes are outside
// this check's guarantee. No system policy setting is modified.
fn execution_policy() -> Result<(), Dependency> {
    fn enabled() -> Result<(), Dependency> {
        let output = native_command("/usr/sbin/spctl", &["--status"], Duration::from_secs(5))?;
        if !super::status_enabled(&output) {
            return failed();
        }
        Ok(())
    }
    enabled()?;
    let output = native_command(
        "/usr/sbin/spctl",
        &["--assess", "--type", "execute", "--raw", install::ROOT],
        Duration::from_secs(20),
    )?;
    assessment_verdict(&output)?;
    // A second public status read catches ordinary changes. This is not an
    // atomic Enforce promise against privileged concurrent policy changes.
    enabled()
}

#[derive(PartialEq, Eq)]
enum Ownership {
    Live,
    Reaped,
    Lost,
}
pub(super) struct Suspended {
    pid: libc::pid_t,
    input: Option<File>,
    output: File,
    error: File,
    deadline: Instant,
    ownership: Ownership,
}
impl Suspended {
    fn poll(&mut self) -> Result<Option<i32>, Dependency> {
        if self.ownership != Ownership::Live {
            return Err(Dependency::OfficialDeviceFailed);
        }
        loop {
            let mut status = 0;
            let rc = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if rc == 0 {
                return Ok(None);
            }
            if rc == self.pid {
                self.ownership = Ownership::Reaped;
                return Ok(Some(status));
            }
            if rc < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            // ECHILD (or any unknown wait failure) loses PID authority. Never
            // signal this numeric PID again; it may already identify another task.
            self.ownership = Ownership::Lost;
            return Err(Dependency::OfficialDeviceFailed);
        }
    }
    fn require_live(&mut self) -> Result<(), Dependency> {
        if self.poll()?.is_some() {
            return Err(Dependency::OfficialDeviceFailed);
        }
        Ok(())
    }

    fn remaining(&self) -> Result<(), Dependency> {
        if Instant::now() < self.deadline {
            Ok(())
        } else {
            Err(Dependency::OfficialDeviceFailed)
        }
    }
    fn start() -> Result<Self, Dependency> {
        let mut signal = unsafe { std::mem::zeroed::<libc::sigaction>() };
        if unsafe { libc::sigaction(libc::SIGCHLD, ptr::null(), &mut signal) } != 0
            || signal.sa_sigaction == libc::SIG_IGN
            || signal.sa_flags & libc::SA_NOCLDWAIT != 0
        {
            return failed();
        }
        fn pipe() -> Result<(File, File), Dependency> {
            let mut fds = [-1; 2];
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                return failed();
            }
            let (read, write) = unsafe { (File::from_raw_fd(fds[0]), File::from_raw_fd(fds[1])) };
            // GUI applications may start with a closed stdio descriptor.
            // Do not let addclose actions later close our dup2 destinations.
            fn above_stdio(file: File) -> Result<File, Dependency> {
                if file.as_raw_fd() >= 3 {
                    return Ok(file);
                }
                let fd = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 3) };
                if fd < 0 {
                    return failed();
                }
                Ok(unsafe { File::from_raw_fd(fd) })
            }
            let read = above_stdio(read)?;
            let write = above_stdio(write)?;
            for fd in [read.as_raw_fd(), write.as_raw_fd()] {
                if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                    return failed();
                }
            }
            Ok((read, write))
        }
        let (input_read, input_write) = pipe()?;
        let (output_read, output_write) = pipe()?;
        let (error_read, error_write) = pipe()?;
        nonblocking(&output_read)?;
        nonblocking(&error_read)?;
        let mut actions = std::mem::MaybeUninit::<libc::posix_spawn_file_actions_t>::uninit();
        let mut attrs = std::mem::MaybeUninit::<libc::posix_spawnattr_t>::uninit();
        if unsafe { libc::posix_spawn_file_actions_init(actions.as_mut_ptr()) } != 0 {
            return failed();
        }
        let mut actions = unsafe { actions.assume_init() };
        if unsafe { libc::posix_spawnattr_init(attrs.as_mut_ptr()) } != 0 {
            unsafe {
                libc::posix_spawn_file_actions_destroy(&mut actions);
            };
            return failed();
        }
        let mut attrs = unsafe { attrs.assume_init() };
        let result = (|| {
            for (fd, target) in [
                (input_read.as_raw_fd(), 0),
                (output_write.as_raw_fd(), 1),
                (error_write.as_raw_fd(), 2),
            ] {
                if unsafe { libc::posix_spawn_file_actions_adddup2(&mut actions, fd, target) } != 0
                {
                    return failed();
                }
            }
            for fd in [
                input_read.as_raw_fd(),
                input_write.as_raw_fd(),
                output_read.as_raw_fd(),
                output_write.as_raw_fd(),
                error_read.as_raw_fd(),
                error_write.as_raw_fd(),
            ] {
                if unsafe { libc::posix_spawn_file_actions_addclose(&mut actions, fd) } != 0 {
                    return failed();
                }
            }
            // Darwin public spawn.h: START_SUSPENDED 0x80, CLOEXEC_DEFAULT 0x4000.
            let flags = libc::c_short::try_from(
                0x0080 | 0x4000 | libc::POSIX_SPAWN_SETPGROUP | libc::POSIX_SPAWN_SETSIGMASK,
            )
            .map_err(|_| Dependency::OfficialDeviceUnverified)?;
            let mut mask = unsafe { std::mem::zeroed::<libc::sigset_t>() };
            if unsafe { libc::sigemptyset(&mut mask) } != 0
                || unsafe { libc::posix_spawnattr_setsigmask(&mut attrs, &mask) } != 0
                || unsafe { libc::posix_spawnattr_setpgroup(&mut attrs, 0) } != 0
                || unsafe { libc::posix_spawnattr_setflags(&mut attrs, flags) } != 0
            {
                return failed();
            }
            let args = [
                CString::new(install::HELPER).unwrap(),
                CString::new("3").unwrap(),
                CString::new("--account-stdin").unwrap(),
            ];
            let env = [
                CString::new("PATH=/usr/bin:/bin").unwrap(),
                CString::new("LANG=C").unwrap(),
            ];
            let mut argv: Vec<*mut c_char> = args.iter().map(|s| s.as_ptr().cast_mut()).collect();
            argv.push(ptr::null_mut());
            let mut envp: Vec<*mut c_char> = env.iter().map(|s| s.as_ptr().cast_mut()).collect();
            envp.push(ptr::null_mut());
            let mut pid = 0;
            if unsafe {
                libc::posix_spawn(
                    &mut pid,
                    args[0].as_ptr(),
                    &actions,
                    &attrs,
                    argv.as_ptr(),
                    envp.as_ptr(),
                )
            } != 0
                || pid <= 0
            {
                return failed();
            }
            Ok(pid)
        })();
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut actions);
            libc::posix_spawnattr_destroy(&mut attrs);
        }
        let pid = result?;
        Ok(Self {
            pid,
            input: Some(input_write),
            output: output_read,
            error: error_read,
            deadline: Instant::now() + Duration::from_secs(15),
            ownership: Ownership::Live,
        })
    }
}
impl Drop for Suspended {
    fn drop(&mut self) {
        // Tokio's normal SIGCHLD handler only wakes its own registered reapers;
        // it is compatible with our separately owned direct child. No code in
        // this flow performs waitpid(-1). External reaping is treated as Lost.
        if self.ownership != Ownership::Live || self.require_live().is_err() {
            return;
        }
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
            libc::kill(self.pid, libc::SIGKILL);
        }
        loop {
            let mut status = 0;
            let rc = unsafe { libc::waitpid(self.pid, &mut status, 0) };
            if rc == self.pid {
                self.ownership = Ownership::Reaped;
                return;
            }
            if rc < 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            self.ownership = Ownership::Lost;
            return;
        }
    }
}

type Ref = *const c_void;
struct Object(Ref);
impl Object {
    fn new(p: Ref) -> Result<Self, Dependency> {
        if p.is_null() {
            failed()
        } else {
            Ok(Self(p))
        }
    }
}
impl Drop for Object {
    fn drop(&mut self) {
        unsafe { CFRelease(self.0) }
    }
}
#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRelease(object: Ref);
    fn CFGetTypeID(object: Ref) -> usize;
    fn CFStringCreateWithCString(allocator: Ref, value: *const c_char, encoding: u32) -> Ref;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringGetCString(value: Ref, buffer: *mut c_char, size: isize, encoding: u32) -> u8;
    fn CFBooleanGetTypeID() -> usize;
    fn CFBooleanGetValue(value: Ref) -> u8;
    fn CFDataCreate(allocator: Ref, bytes: *const u8, length: isize) -> Ref;
    fn CFPropertyListCreateWithData(
        allocator: Ref,
        data: Ref,
        options: usize,
        format: *mut isize,
        error: *mut Ref,
    ) -> Ref;
}
fn string(s: &str) -> Result<Object, Dependency> {
    let s = CString::new(s).map_err(|_| Dependency::OfficialDeviceUnverified)?;
    Object::new(unsafe { CFStringCreateWithCString(ptr::null(), s.as_ptr(), 0x08000100) })
}
fn assessment_verdict(bytes: &[u8]) -> Result<(), Dependency> {
    let data =
        Object::new(unsafe { CFDataCreate(ptr::null(), bytes.as_ptr(), bytes.len() as isize) })?;
    let mut error = ptr::null();
    let raw = unsafe {
        CFPropertyListCreateWithData(ptr::null(), data.0, 0, ptr::null_mut(), &mut error)
    };
    if !error.is_null() {
        unsafe { CFRelease(error) };
        if !raw.is_null() {
            unsafe { CFRelease(raw) };
        }
        return failed();
    }
    let result = Object::new(raw)?;
    if unsafe { CFGetTypeID(result.0) } != unsafe { CFDictionaryGetTypeID() } {
        return failed();
    }
    let verdict = string("assessment:verdict")?;
    let verdict = unsafe { CFDictionaryGetValue(result.0, verdict.0) };
    if verdict.is_null()
        || unsafe { CFGetTypeID(verdict) } != unsafe { CFBooleanGetTypeID() }
        || unsafe { CFBooleanGetValue(verdict) } == 0
    {
        return failed();
    }
    let authority_key = string("assessment:authority")?;
    let authority = unsafe { CFDictionaryGetValue(result.0, authority_key.0) };
    if authority.is_null()
        || unsafe { CFGetTypeID(authority) } != unsafe { CFDictionaryGetTypeID() }
    {
        return failed();
    }
    for key in ["assessment:error", "assessment:cserror"] {
        let key = string(key)?;
        if !unsafe { CFDictionaryGetValue(result.0, key.0) }.is_null() {
            return failed();
        }
    }
    let override_key = string("assessment:authority:override")?;
    let weak_key = string("assessment:authority:weak")?;
    let overridden = !unsafe { CFDictionaryGetValue(authority, override_key.0) }.is_null()
        || !unsafe { CFDictionaryGetValue(authority, weak_key.0) }.is_null();
    let source_key = string("assessment:authority:source")?;
    let source = unsafe { CFDictionaryGetValue(authority, source_key.0) };
    if source.is_null() || unsafe { CFGetTypeID(source) } != unsafe { CFStringGetTypeID() } {
        return failed();
    }
    let mut text = [0u8; 256];
    if unsafe {
        CFStringGetCString(
            source,
            text.as_mut_ptr().cast(),
            text.len() as isize,
            0x08000100,
        )
    } == 0
    {
        return failed();
    }
    let end = text
        .iter()
        .position(|b| *b == 0)
        .ok_or(Dependency::OfficialDeviceUnverified)?;
    let source =
        std::str::from_utf8(&text[..end]).map_err(|_| Dependency::OfficialDeviceUnverified)?;
    if !super::accepts_policy(Some(true), Some(source), overridden) {
        return failed();
    }
    Ok(())
}
