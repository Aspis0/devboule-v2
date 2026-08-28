//! Pipe DACL: current user only. The Windows default for a named pipe is not
//! safe enough for a process that will later hold provider credentials.

use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;

#[cfg(feature = "server")]
use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{GetLastError, LocalFree, HANDLE, INVALID_HANDLE_VALUE};
#[cfg(feature = "server")]
use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
use windows_sys::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSidToSidW, GetSecurityInfo, SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    EqualSid, GetTokenInformation, TokenUser, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, TOKEN_QUERY, TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

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
    unsafe { token_user_sid(current_process_token()?) }
}

/// Resolve the user SID from a peer process rather than from a value supplied
/// by that process in a protocol frame.
pub fn process_user_sid(pid: u32) -> io::Result<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return Err(last_os_error());
        }
        let process = OwnedHandle::from_raw_handle(process as RawHandle);
        token_user_sid(open_process_token(process.as_raw_handle() as HANDLE)?)
    }
}

unsafe fn current_process_token() -> io::Result<OwnedHandle> {
    open_process_token(GetCurrentProcess())
}

unsafe fn open_process_token(process: HANDLE) -> io::Result<OwnedHandle> {
    let mut token: HANDLE = INVALID_HANDLE_VALUE;
    if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
        return Err(last_os_error());
    }
    Ok(OwnedHandle::from_raw_handle(token as RawHandle))
}

unsafe fn token_user_sid(token: OwnedHandle) -> io::Result<String> {
    let token = token;

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

/// Test-only oracle; production code never calls this function. The daemon
/// applies [`user_only_sddl`] when creating the pipe and does not re-check its
/// DACL at runtime. This verifies that the supplied SDDL has at least one ACE
/// and that every ACE trustee resolves to `sid`; it does not inspect a live
/// object or prove any other property of Windows access evaluation.
pub fn dacl_is_current_user_only(sddl: &str, sid: &str) -> bool {
    let Some(trustees) = dacl_trustees(sddl) else {
        return false;
    };
    let Some(current_sid) = sid_from_string(sid) else {
        return false;
    };

    trustees.iter().all(|trustee| {
        let Some(trustee_sid) = sid_from_string(trustee) else {
            return false;
        };
        unsafe { EqualSid(current_sid.as_ptr(), trustee_sid.as_ptr()) != 0 }
    })
}

fn dacl_trustees(sddl: &str) -> Option<Vec<&str>> {
    let dacl_start = sddl.find("D:")? + 2;
    let dacl_tail = &sddl[dacl_start..];
    let dacl_end = next_sddl_section(dacl_tail);
    let dacl = &dacl_tail[..dacl_end];

    let mut trustees = Vec::new();
    let mut depth = 0usize;
    let mut ace_start = None;

    for (offset, character) in dacl.char_indices() {
        match character {
            '(' => {
                if depth == 0 {
                    ace_start = Some(offset);
                }
                depth += 1;
            }
            ')' => {
                if depth == 0 {
                    return None;
                }
                depth -= 1;
                if depth == 0 {
                    let start = ace_start.take()?;
                    let fields: Vec<_> = dacl[start + 1..offset].split(';').collect();
                    trustees.push(fields.get(5)?.trim());
                }
            }
            _ => {}
        }
    }

    (depth == 0 && ace_start.is_none() && !trustees.is_empty()).then_some(trustees)
}

fn next_sddl_section(sddl: &str) -> usize {
    let mut depth = 0usize;
    for (offset, character) in sddl.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            'O' | 'G' | 'D' | 'S'
                if depth == 0 && offset > 0 && sddl.as_bytes().get(offset + 1) == Some(&b':') =>
            {
                return offset;
            }
            _ => {}
        }
    }
    sddl.len()
}

struct LocalSid(PSID);

impl LocalSid {
    fn as_ptr(&self) -> PSID {
        self.0
    }
}

impl Drop for LocalSid {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0 as _);
        }
    }
}

fn sid_from_string(sid: &str) -> Option<LocalSid> {
    let sid_wide = wide(sid);
    let mut sid_ptr: PSID = ptr::null_mut();
    let ok = unsafe { ConvertStringSidToSidW(sid_wide.as_ptr(), &mut sid_ptr) };
    (ok != 0 && !sid_ptr.is_null()).then_some(LocalSid(sid_ptr))
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
        assert!(!dacl_is_current_user_only(
            &format!("D:(A;;GA;;;{sid})(A;;GA;;;NU)"),
            sid
        ));
        assert!(!dacl_is_current_user_only(
            &format!("D:(A;;GA;;;{sid})(A;;GA;;;SY)"),
            sid
        ));
    }

    #[test]
    fn alias_trustee_is_the_current_user() {
        let sid = sid_for_alias("LA");
        assert!(
            dacl_is_current_user_only("D:(A;;GA;;;LA)", &sid),
            "an SDDL alias for the trustee must identify the same user: SID={sid}"
        );
    }

    #[test]
    fn unrelated_user_trustee_is_not_current_user_only() {
        let sid = "S-1-5-21-1-2-3-1001";
        let other_sid = "S-1-5-21-1-2-3-1002";
        assert!(!dacl_is_current_user_only(
            &format!("D:(A;;GA;;;{sid})(A;;GA;;;{other_sid})"),
            sid
        ));
    }

    fn sid_for_alias(alias: &str) -> String {
        unsafe {
            let alias_wide = wide(alias);
            let mut sid = ptr::null_mut();
            assert_ne!(
                ConvertStringSidToSidW(alias_wide.as_ptr(), &mut sid),
                0,
                "ConvertStringSidToSidW({alias}) failed: {}",
                last_os_error()
            );

            let mut sid_text = ptr::null_mut();
            assert_ne!(
                ConvertSidToStringSidW(sid, &mut sid_text),
                0,
                "ConvertSidToStringSidW failed: {}",
                last_os_error()
            );
            let result = pwstr_to_string(sid_text).expect("alias SID is valid UTF-16");
            LocalFree(sid_text as _);
            LocalFree(sid as _);
            result
        }
    }

    #[test]
    fn current_user_sid_looks_like_a_sid() {
        let sid = current_user_sid().expect("sid");
        assert!(sid.starts_with("S-1-"), "{sid}");
    }
}
