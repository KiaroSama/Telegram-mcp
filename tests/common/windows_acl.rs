//! A Windows access-denial fixture that can restore its exact original DACL
//! without reopening the now-inaccessible directory.
use std::{
    ffi::c_void,
    fs::{File, OpenOptions},
    io,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HANDLE, LocalFree},
    Security::{
        ACL,
        Authorization::{
            ConvertStringSidToSidW, DENY_ACCESS, EXPLICIT_ACCESS_W, GetSecurityInfo,
            NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetSecurityInfo, TRUSTEE_IS_SID,
            TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
        },
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorControl, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    },
    Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL, WRITE_DAC,
    },
};

struct LocalAllocation(*mut c_void);
impl Drop for LocalAllocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: every allocation is returned by a LocalAlloc-based API
            // below and is owned by this guard exactly once.
            unsafe { LocalFree(self.0) };
        }
    }
}

struct Dacl {
    _descriptor: LocalAllocation,
    acl: *mut ACL,
    protected: bool,
}
impl Dacl {
    fn read(handle: HANDLE) -> io::Result<Self> {
        let mut descriptor = null_mut();
        let mut acl = null_mut();
        // SAFETY: handle is held open; all output pointers refer to initialized
        // local slots. The returned ACL is owned by the returned descriptor.
        status(unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut acl,
                null_mut(),
                &mut descriptor,
            )
        })?;
        let descriptor = LocalAllocation(descriptor);
        if acl.is_null() {
            return Err(io::Error::other("Fixture unexpectedly has a null DACL"));
        }
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: descriptor is a live OS-provided security descriptor.
        if unsafe { GetSecurityDescriptorControl(descriptor.0, &mut control, &mut revision) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            _descriptor: descriptor,
            acl,
            protected: control & SE_DACL_PROTECTED != 0,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        // SAFETY: GetSecurityInfo returned a valid ACL whose AclSize bytes stay
        // alive with its owning security descriptor for the lifetime of self.
        unsafe { std::slice::from_raw_parts(self.acl.cast::<u8>(), (*self.acl).AclSize as usize) }
            .to_vec()
    }
}

pub struct ReadDenied {
    handle: File,
    original: Dacl,
    active: bool,
}
impl ReadDenied {
    pub fn new(path: &Path) -> io::Result<Self> {
        let handle = OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(path)?;
        let original = Dacl::read(handle.as_raw_handle())?;
        let mut guard = Self {
            handle,
            original,
            active: false,
        };
        let owner_rights: Vec<u16> = "S-1-3-4\0".encode_utf16().collect();
        let mut sid = null_mut();
        // SAFETY: owner_rights is nul-terminated; sid is a valid output slot.
        if unsafe { ConvertStringSidToSidW(owner_rights.as_ptr(), &mut sid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = LocalAllocation(sid);
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
            grfAccessMode: DENY_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: TRUSTEE_W {
                pMultipleTrustee: null_mut(),
                MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
                TrusteeForm: TRUSTEE_IS_SID,
                TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
                ptstrName: sid.0.cast(),
            },
        };
        let mut acl = null_mut();
        // SAFETY: entry's SID and the original ACL are live throughout the call.
        status(unsafe { SetEntriesInAclW(1, &entry, guard.original.acl, &mut acl) })?;
        let acl = LocalAllocation(acl.cast());
        guard.active = true;
        // SAFETY: the handle retains WRITE_DAC, and the OS-created ACL is live.
        status(unsafe {
            SetSecurityInfo(
                guard.handle.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                acl.0.cast(),
                null(),
            )
        })?;
        Ok(guard)
    }

    pub fn current_acl(&self) -> io::Result<Vec<u8>> {
        Ok(Dacl::read(self.handle.as_raw_handle())?.bytes())
    }

    pub fn original_acl(&self) -> Vec<u8> {
        self.original.bytes()
    }

    pub fn restore(&mut self) -> io::Result<()> {
        if !self.active {
            return Ok(());
        }
        let inheritance = if self.original.protected {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        // SAFETY: the pre-denial handle retains WRITE_DAC. The original ACL
        // remains alive, and null owner/group/SACL pointers are not selected.
        status(unsafe {
            SetSecurityInfo(
                self.handle.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | inheritance,
                null_mut(),
                null_mut(),
                self.original.acl,
                null(),
            )
        })?;
        self.active = false;
        Ok(())
    }
}
impl Drop for ReadDenied {
    fn drop(&mut self) {
        // Restore even when an assertion unwinds; the fixture must not leave
        // inaccessible directories behind on the CI runner.
        let _ = self.restore();
    }
}

fn status(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
