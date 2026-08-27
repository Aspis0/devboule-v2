//! Pipe DACL: current user only. The Windows default for a named pipe is not
//! safe enough for a process that will later hold provider credentials.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;

#[cfg(feature = "server")]
use windows_sys::Win32::Foundation::BOOL;
use windows_sys::Win32::Foundation::{GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(feature = "server")]
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

#[cfg(feature = "server")]
pub struct PipeSecurity {
    descriptor: PSECURITY_DESCRIPTOR,
}

// The descriptor is owned, never aliased across threads except by moving the
// listener into the accept thread before any accept runs.
#[cfg(feature = "server")]
unsafe impl Send for PipeSecurity {}

#[cfg(feature = "server")]
impl PipeSecurity {
    pub fn current_user_only() -> io::Result<Self> {
        let sid = current_user_sid()?;
        let sddl = user_only_sddl(&sid);
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let wide = wide(&sddl);
        let ok: BOOL = unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        };
        if ok == 0 || descriptor.is_null() {
            return Err(last_os_error());
        }
        Ok(Self { descriptor })
    }

    pub fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.descriptor
    }
}

#[cfg(feature = "server")]
impl Drop for PipeSecurity {
    fn drop(&mut self) {
        if !self.descriptor.is_null() {
            unsafe {
                LocalFree(self.descriptor as _);
            }
            self.descriptor = ptr::null_mut();
        }
    }
}

pub fn user_only_sddl(sid: &str) -> String {
    format!("O:{sid}D:P(A;;GA;;;{sid})")
}

pub fn current_user_sid() -> io::Result<String> {
    unsafe {
        let mut token: HANDLE = INVALID_HANDLE_VALUE;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(last_os_error());
        }
        let token = OwnedHandle::from_raw_handle(token as RawHandle);

        let mut needed = 0u32;
        GetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenUser,
            ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 {
            return Err(last_os_error());
        }
        let mut buf = vec![0u8; needed as usize];
        if GetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenUser,
            buf.as_mut_ptr().cast(),
            needed,
            &mut needed,
        ) == 0
        {
            return Err(last_os_error());
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut sid_str = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid_str) == 0 {
            return Err(last_os_error());
        }
        let text = pwstr_to_string(sid_str)?;
        LocalFree(sid_str as _);
        Ok(text)
    }
}

/// DACL of a kernel object (the named pipe) as SDDL. Used to verify we did
/// not leave the pipe world-readable.
pub fn dacl_sddl(handle: HANDLE) -> io::Result<String> {
    unsafe {
        let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
        let status = GetSecurityInfo(
            handle,
            SE_KERNEL_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut sd,
        );
        if status != 0 {
            return Err(io::Error::from_raw_os_error(status as i32));
        }
        let mut text = ptr::null_mut();
        let ok = ConvertSecurityDescriptorToStringSecurityDescriptorW(
            sd,
            SDDL_REVISION_1,
            DACL_SECURITY_INFORMATION,
            &mut text,
            ptr::null_mut(),
        );
        LocalFree(sd as _);
        if ok == 0 {
            return Err(last_os_error());
        }
        let sddl = pwstr_to_string(text)?;
        LocalFree(text as _);
        Ok(sddl)
    }
}

pub fn dacl_is_current_user_only(sddl: &str, sid: &str) -> bool {
    let has_user = sddl.contains(sid);
    let open_trustees = [
        ";;;WD)",
        ";;;BU)",
        ";;;AU)",
        ";;;AN)",
        ";;;BA)",
        ";;;S-1-1-0)",
    ];
    has_user && open_trustees.iter().all(|trustee| !sddl.contains(trustee))
}

pub fn last_os_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { GetLastError() } as i32)
}

pub fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn pwstr_to_string(ptr: *const u16) -> io::Result<String> {
    if ptr.is_null() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "null wide string",
        ));
    }
    let mut len = 0usize;
    while *ptr.add(len) != 0 {
        len += 1;
        if len > 4096 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "wide string too long",
            ));
        }
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    OsString::from_wide(slice)
        .into_string()
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "sid is not unicode"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_only_sddl_is_protected_generic_all_for_one_sid() {
        let sid = "S-1-5-21-1-2-3-1001";
        let sddl = user_only_sddl(sid);
        assert!(sddl.starts_with("O:S-1-5-21-1-2-3-1001D:P(A;;GA;;;"));
        assert!(dacl_is_current_user_only(
            &format!("D:P(A;;GA;;;{sid})"),
            sid
        ));
        assert!(!dacl_is_current_user_only(
            &format!("D:(A;;GA;;;{sid})(A;;GA;;;WD)"),
            sid
        ));
    }

    #[test]
    fn current_user_sid_looks_like_a_sid() {
        let sid = current_user_sid().expect("sid");
        assert!(sid.starts_with("S-1-"), "{sid}");
    }
}
