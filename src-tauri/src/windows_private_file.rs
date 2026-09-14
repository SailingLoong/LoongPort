//! Windows DACL boundary for application-owned credential files.

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, LocalFree, ERROR_INSUFFICIENT_BUFFER, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    GetSecurityInfo, SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, SET_ACCESS,
    SE_FILE_OBJECT, SUB_CONTAINERS_AND_INHERIT, SUB_OBJECTS_AND_INHERIT, TRUSTEE_IS_SID,
    TRUSTEE_IS_USER, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    AclSizeInformation, CreateWellKnownSid, EqualSid, GetAce, GetAclInformation,
    GetSecurityDescriptorControl, GetTokenInformation, InitializeSecurityDescriptor,
    SetSecurityDescriptorControl, SetSecurityDescriptorDacl, TokenUser,
    WinBuiltinAdministratorsSid, WinLocalSystemSid, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
    ACL_SIZE_INFORMATION, DACL_SECURITY_INFORMATION, INHERITED_ACE, NO_INHERITANCE,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    SECURITY_DESCRIPTOR, SECURITY_MAX_SID_SIZE, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, CREATE_NEW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, SECURITY_DESCRIPTOR_REVISION,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// VACUUM 等以改名重建文件后的瞬间，按名新建句柄可能短暂收到 ACCESS_DENIED
/// （名字空间沉降 / 杀软扫描窗口）。有界重试覆盖该瞬态；重试耗尽仍失败才报错，
/// 真实的权限问题不会被吞掉。
fn settle_retry<T>(mut action: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    for attempt in 0..5u32 {
        let result = action();
        let transient =
            matches!(&result, Err(error) if error.kind() == io::ErrorKind::PermissionDenied);
        if !transient || attempt == 4 {
            return result;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    unreachable!("settle_retry returns on the final attempt");
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: OpenProcessToken returned this owned handle and it is closed once here.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct LocalAllocation(*mut core::ffi::c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: this buffer was allocated by a Windows API that documents LocalFree.
        unsafe {
            LocalFree(self.0);
        }
    }
}

fn current_user_token() -> io::Result<(OwnedHandle, Vec<usize>)> {
    let mut token = std::ptr::null_mut();
    // SAFETY: the process pseudo-handle is valid and token points to writable storage.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = OwnedHandle(token);

    let mut required = 0;
    // SAFETY: a null output buffer with zero length is the documented sizing call.
    let sized =
        unsafe { GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &mut required) };
    let sizing_error = io::Error::last_os_error();
    if sized != 0
        || sizing_error.raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
        || required == 0
    {
        return Err(sizing_error);
    }

    let word = std::mem::size_of::<usize>();
    let mut buffer = vec![0usize; (required as usize).div_ceil(word)];
    // SAFETY: the usize buffer is aligned for TOKEN_USER and has at least `required` bytes.
    if unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok((token, buffer))
}

fn token_user_sid(buffer: &mut [usize]) -> PSID {
    // SAFETY: current_user_token returns an aligned TOKEN_USER buffer and its embedded SID
    // remains valid for the lifetime of the buffer.
    unsafe { (*(buffer.as_mut_ptr().cast::<TOKEN_USER>())).User.Sid }
}

fn well_known_sid(kind: i32) -> io::Result<[u8; SECURITY_MAX_SID_SIZE as usize]> {
    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut size = sid.len() as u32;
    // SAFETY: sid is a writable SECURITY_MAX_SID_SIZE buffer and size describes it.
    if unsafe {
        CreateWellKnownSid(
            kind,
            std::ptr::null_mut(),
            sid.as_mut_ptr().cast(),
            &mut size,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(sid)
}

fn allow_full_access(sid: PSID, trustee_type: i32, inheritance: u32) -> EXPLICIT_ACCESS_W {
    let trustee = TRUSTEE_W {
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: trustee_type,
        ptstrName: sid.cast(),
        ..Default::default()
    };
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_ALL_ACCESS,
        grfAccessMode: SET_ACCESS,
        grfInheritance: inheritance,
        Trustee: trustee,
    }
}

/// 文件自身的受限 DACL：仅当前用户、SYSTEM 与本机管理员，且不向子对象继承。
fn private_acl(
    user_sid: PSID,
    system_sid: PSID,
    administrators_sid: PSID,
) -> io::Result<LocalAllocation> {
    private_acl_with(user_sid, system_sid, administrators_sid, NO_INHERITANCE)
}

/// 目录的受限 DACL。ACE 可继承：SQLite VACUUM 会以改名重建数据库文件，重建
/// 产物带进程默认 DACL——可继承的目录 ACE 保证这类重建文件自动获得同等保护，
/// 不需要每个重建点各自补救。
fn private_acl_inheritable(
    user_sid: PSID,
    system_sid: PSID,
    administrators_sid: PSID,
) -> io::Result<LocalAllocation> {
    private_acl_with(
        user_sid,
        system_sid,
        administrators_sid,
        SUB_CONTAINERS_AND_INHERIT | SUB_OBJECTS_AND_INHERIT,
    )
}

fn private_acl_with(
    user_sid: PSID,
    system_sid: PSID,
    administrators_sid: PSID,
    inheritance: u32,
) -> io::Result<LocalAllocation> {
    let entries = [
        allow_full_access(user_sid, TRUSTEE_IS_USER, inheritance),
        allow_full_access(system_sid, TRUSTEE_IS_WELL_KNOWN_GROUP, inheritance),
        allow_full_access(administrators_sid, TRUSTEE_IS_WELL_KNOWN_GROUP, inheritance),
    ];
    let mut acl = std::ptr::null_mut();
    // SAFETY: all trustee SID buffers remain alive for this call; OldAcl is null so the
    // resulting ACL contains only these explicit entries.
    let result = unsafe {
        SetEntriesInAclW(
            entries.len() as u32,
            entries.as_ptr(),
            std::ptr::null(),
            &mut acl,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    if acl.is_null() {
        return Err(invalid_private_acl("Windows returned an empty private ACL"));
    }
    Ok(LocalAllocation(acl.cast()))
}

fn invalid_private_acl(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, message)
}

fn verify_private_dacl(
    handle: HANDLE,
    user_sid: PSID,
    system_sid: PSID,
    administrators_sid: PSID,
) -> io::Result<()> {
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
    // SAFETY: handle is valid and every requested output pointer refers to writable storage.
    let security_result = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    if security_result != 0 {
        return Err(io::Error::from_raw_os_error(security_result as i32));
    }
    if descriptor.is_null() {
        return Err(invalid_private_acl(
            "Windows returned no security descriptor for a private file",
        ));
    }
    let _descriptor = LocalAllocation(descriptor);
    if dacl.is_null() {
        return Err(invalid_private_acl(
            "Windows returned no DACL for a private file",
        ));
    }

    let mut control = 0;
    let mut revision = 0;
    // SAFETY: descriptor remains alive and both output pointers are writable.
    if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if control & SE_DACL_PROTECTED == 0 {
        return Err(invalid_private_acl("private file DACL is not protected"));
    }

    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: dacl belongs to the live descriptor and size is a correctly sized output buffer.
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if size.AceCount != 3 {
        return Err(invalid_private_acl(
            "private file DACL has unexpected access entries",
        ));
    }

    let expected = [user_sid, system_sid, administrators_sid];
    let mut found = [false; 3];
    for index in 0..size.AceCount {
        let mut raw_ace = std::ptr::null_mut();
        // SAFETY: index is bounded by the DACL's reported ACE count.
        if unsafe { GetAce(dacl, index, &mut raw_ace) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: GetAce returned a valid pointer to an ACE header in the live DACL.
        let header = unsafe { &*raw_ace.cast::<ACE_HEADER>() };
        if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE
            || header.AceFlags as u32 & INHERITED_ACE != 0
            || usize::from(header.AceSize) < std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        {
            return Err(invalid_private_acl(
                "private file DACL contains an unexpected access entry",
            ));
        }
        // SAFETY: the checked ACE type and size describe an ACCESS_ALLOWED_ACE.
        let ace = unsafe { &*raw_ace.cast::<ACCESS_ALLOWED_ACE>() };
        if ace.Mask != FILE_ALL_ACCESS {
            return Err(invalid_private_acl(
                "private file DACL grants unexpected access rights",
            ));
        }
        let sid: PSID = (&ace.SidStart as *const u32).cast_mut().cast();
        let mut matched = false;
        for (slot, expected_sid) in found.iter_mut().zip(expected) {
            // SAFETY: both pointers refer to SIDs in live buffers or the live descriptor.
            if unsafe { EqualSid(sid, expected_sid) } != 0 {
                if *slot {
                    return Err(invalid_private_acl(
                        "private file DACL contains a duplicate access entry",
                    ));
                }
                *slot = true;
                matched = true;
                break;
            }
        }
        if !matched {
            return Err(invalid_private_acl(
                "private file DACL grants access to an unexpected principal",
            ));
        }
    }
    if !found.into_iter().all(|present| present) {
        return Err(invalid_private_acl(
            "private file DACL is missing a required access entry",
        ));
    }
    Ok(())
}

/// Creates a new, unshared file whose protected DACL allows only the active user,
/// LocalSystem, and local administrators. The DACL is installed by CreateFileW and
/// verified before the handle is returned, so callers cannot write credential bytes first.
pub(crate) fn create_new(path: &Path) -> io::Result<std::fs::File> {
    let (_token, mut user_buffer) = current_user_token()?;
    let user_sid = token_user_sid(&mut user_buffer);
    let mut system_sid = well_known_sid(WinLocalSystemSid)?;
    let mut administrators_sid = well_known_sid(WinBuiltinAdministratorsSid)?;
    let system_sid: PSID = system_sid.as_mut_ptr().cast();
    let administrators_sid: PSID = administrators_sid.as_mut_ptr().cast();
    let acl = private_acl(user_sid, system_sid, administrators_sid)?;

    let mut descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_ptr = (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast();
    // SAFETY: descriptor_ptr points to writable SECURITY_DESCRIPTOR storage.
    if unsafe { InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: descriptor is initialized and acl remains alive through CreateFileW.
    if unsafe { SetSecurityDescriptorDacl(descriptor_ptr, 1, acl.0.cast(), 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: descriptor is initialized; setting SE_DACL_PROTECTED prevents inherited ACEs.
    if unsafe { SetSecurityDescriptorControl(descriptor_ptr, SE_DACL_PROTECTED, SE_DACL_PROTECTED) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }

    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor_ptr,
        bInheritHandle: 0,
    };
    let path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: path_wide is NUL-terminated; security descriptor, DACL, and SID buffers remain
    // alive for the call. CREATE_NEW prevents replacement and share mode zero denies sharing.
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            FILE_GENERIC_WRITE,
            0,
            &attributes,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned a unique owned handle that File will close exactly once.
    let file = unsafe { std::fs::File::from_raw_handle(handle) };
    if let Err(error) = verify_private_dacl(
        file.as_raw_handle(),
        user_sid,
        system_sid,
        administrators_sid,
    ) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(file)
}

/// Tighten an existing app-owned file or directory and verify the effective DACL.
pub(crate) fn restrict_existing(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(invalid_private_acl(
            "private path is not the expected regular file type",
        ));
    }

    let (_token, mut user_buffer) = current_user_token()?;
    let user_sid = token_user_sid(&mut user_buffer);
    let mut system_sid = well_known_sid(WinLocalSystemSid)?;
    let mut administrators_sid = well_known_sid(WinBuiltinAdministratorsSid)?;
    let system_sid: PSID = system_sid.as_mut_ptr().cast();
    let administrators_sid: PSID = administrators_sid.as_mut_ptr().cast();
    let acl = if directory {
        private_acl_inheritable(user_sid, system_sid, administrators_sid)?
    } else {
        private_acl(user_sid, system_sid, administrators_sid)?
    };
    let mut path_wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: path_wide is writable NUL-terminated UTF-16 and the ACL remains alive.
    // 目标文件可能刚被 VACUUM/改名重建，名字层面的操作走沉降重试。
    let flags = if directory {
        FILE_FLAG_BACKUP_SEMANTICS
    } else {
        FILE_ATTRIBUTE_NORMAL
    };
    settle_retry(|| {
        let security_result = unsafe {
            SetNamedSecurityInfoW(
                path_wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                acl.0.cast(),
                std::ptr::null(),
            )
        };
        if security_result != 0 {
            return Err(io::Error::from_raw_os_error(security_result as i32));
        }

        // SAFETY: path_wide remains NUL-terminated; the returned handle is checked and owned below.
        let handle = unsafe {
            CreateFileW(
                path_wide.as_ptr(),
                FILE_GENERIC_READ,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                std::ptr::null(),
                OPEN_EXISTING,
                flags,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateFileW returned a unique owned handle that File will close exactly once.
        let file = unsafe { std::fs::File::from_raw_handle(handle) };
        verify_private_dacl(
            file.as_raw_handle(),
            user_sid,
            system_sid,
            administrators_sid,
        )
    })
}

/// 打开已存在的文件并 fsync。刚被 VACUUM/改名重建的文件按名打开可能撞上
/// 短暂 ACCESS_DENIED，走沉降重试。
pub(crate) fn sync_file(path: &Path) -> io::Result<()> {
    settle_retry(|| {
        let file = std::fs::File::open(path)?;
        file.sync_all()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_world_full_access(path: &Path) {
        let mut world = well_known_sid(windows_sys::Win32::Security::WinWorldSid).unwrap();
        let entry = allow_full_access(
            world.as_mut_ptr().cast(),
            TRUSTEE_IS_WELL_KNOWN_GROUP,
            NO_INHERITANCE,
        );
        let mut acl = std::ptr::null_mut();
        // SAFETY: the world SID remains alive and the output ACL pointer is writable.
        assert_eq!(
            unsafe { SetEntriesInAclW(1, &entry, std::ptr::null(), &mut acl) },
            0
        );
        assert!(!acl.is_null());
        let acl = LocalAllocation(acl.cast());
        let mut path_wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: path_wide is NUL-terminated and the ACL is alive for the call.
        assert_eq!(
            unsafe {
                SetNamedSecurityInfoW(
                    path_wide.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    acl.0.cast(),
                    std::ptr::null(),
                )
            },
            0
        );
    }

    fn assert_restricted_dacl(path: &Path) {
        let file = std::fs::File::open(path).unwrap();
        let (_token, mut user_buffer) = current_user_token().unwrap();
        let user_sid = token_user_sid(&mut user_buffer);
        let mut system = well_known_sid(WinLocalSystemSid).unwrap();
        let mut administrators = well_known_sid(WinBuiltinAdministratorsSid).unwrap();
        verify_private_dacl(
            file.as_raw_handle(),
            user_sid,
            system.as_mut_ptr().cast(),
            administrators.as_mut_ptr().cast(),
        )
        .unwrap();
    }

    #[test]
    fn private_atomic_write_replaces_permissive_dacl_with_restricted_dacl() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("credential.json");
        std::fs::write(&path, b"old credential").unwrap();
        set_world_full_access(&path);

        super::super::atomic_write_private(&path, b"new credential").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"new credential");
        assert_restricted_dacl(&path);
    }

    /// 目录收紧后，VACUUM/改名重建出的文件必须自动继承同等保护，
    /// 而不是落到进程默认 DACL 上。
    #[test]
    fn restricted_directory_grants_children_inherited_access() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("private-root");
        std::fs::create_dir(&root).unwrap();

        restrict_existing(&root, true).unwrap();

        // 模拟 SQLite VACUUM：在受保护目录里新建一个未指定 DACL 的文件。
        let rebuilt = root.join("loongport.db");
        std::fs::write(&rebuilt, b"rebuilt").unwrap();
        assert_eq!(std::fs::read(&rebuilt).unwrap(), b"rebuilt");

        // 继承来的 ACE 必须把当前用户（含 FILE_ALL_ACCESS）列进 DACL。
        // 生产校验器拒绝继承 ACE（要求文件自身 PROTECTED），这里单独按继承形状断言。
        let file = std::fs::File::open(&rebuilt).unwrap();
        let mut dacl: *mut ACL = std::ptr::null_mut();
        let mut descriptor: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        assert_eq!(
            unsafe {
                GetSecurityInfo(
                    file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut dacl,
                    std::ptr::null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let _descriptor = LocalAllocation(descriptor);
        let mut size = ACL_SIZE_INFORMATION::default();
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );
        assert!(
            size.AceCount >= 3,
            "child inherits the restricted directory entries"
        );
        let (_token, mut user_buffer) = current_user_token().unwrap();
        let user_sid = token_user_sid(&mut user_buffer);
        let mut matched_user = false;
        for index in 0..size.AceCount {
            let mut raw_ace = std::ptr::null_mut();
            assert_ne!(unsafe { GetAce(dacl, index, &mut raw_ace) }, 0);
            let ace = unsafe { &*raw_ace.cast::<ACCESS_ALLOWED_ACE>() };
            let sid: PSID = (&ace.SidStart as *const u32).cast_mut().cast();
            if unsafe { EqualSid(sid, user_sid) } != 0 {
                assert_eq!(ace.Mask, FILE_ALL_ACCESS);
                assert_ne!(
                    unsafe { &*raw_ace.cast::<ACE_HEADER>() }.AceFlags as u32 & INHERITED_ACE,
                    0,
                    "child entry for the user must be inherited from the directory"
                );
                matched_user = true;
            }
        }
        assert!(matched_user, "child DACL includes the current user");
    }
}
