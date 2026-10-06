//! The Windows queries behind the process index: members from the session's
//! Job Object, identity from an opened handle, parents and listener ports
//! from one machine-wide snapshot per refresh.
//!
//! Windows cannot read another process's command line through a documented
//! API, so `argv` is empty here and an entry carries its exe only — the
//! macOS half is the one that can show argv, and the report records this.

use std::collections::HashMap;
use std::mem;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_NO_DATA, FILETIME,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::{CreationStatus, ProcessIdentity, FILETIME_EPOCH_MS};
use crate::process_tree::JobObject;

pub(crate) const PROOF_KIND: &str = "job_member";

/// The platform probe: one `begin_refresh` snapshot answers the whole
/// refresh — every pid's parent from a single Toolhelp walk (not one walk
/// per member) and both listener tables.
pub(crate) struct Probe {
    parents: HashMap<u32, u32>,
    ports: Vec<(u16, u32)>,
}

impl Probe {
    pub(crate) fn new() -> Self {
        Self {
            parents: HashMap::new(),
            ports: Vec::new(),
        }
    }

    pub(crate) fn begin_refresh(&mut self) -> Result<(), String> {
        self.parents = process_parents().ok_or("process snapshot unavailable")?;
        self.ports = listener_ports().ok_or("listener table unavailable")?;
        Ok(())
    }

    pub(crate) fn members(&self, job: &JobObject) -> Vec<u32> {
        // A failed query means the job handle is gone (the session ended), and a
        // dead job has no members to claim.
        job.pids().unwrap_or_default()
    }

    pub(crate) fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        // The snapshot is the vouch: a member it never saw has no parent to
        // report, and an unparented member is never a cleanup target.
        let ppid = *self.parents.get(&pid)?;
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let started_at_ms = read_creation_time(handle);
        let exe = read_exe(handle);
        unsafe { CloseHandle(handle) };
        Some(ProcessIdentity {
            started_at_ms: started_at_ms?,
            ppid,
            exe,
            argv: Vec::new(),
        })
    }

    pub(crate) fn listening_ports(&self) -> Vec<(u16, u32)> {
        self.ports.clone()
    }
}

/// The creation-time read the terminate-time identity check uses: the same
/// clock identity the index records, re-read a moment before a signal.
pub(crate) fn creation_status(pid: u32) -> CreationStatus {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        let error = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        return if error == ERROR_INVALID_PARAMETER {
            CreationStatus::Gone
        } else {
            CreationStatus::Unverified
        };
    }
    let status = match read_creation_time(handle) {
        Some(started_at_ms) => CreationStatus::At(started_at_ms),
        None => CreationStatus::Unverified,
    };
    unsafe { CloseHandle(handle) };
    status
}

fn read_creation_time(handle: HANDLE) -> Option<u64> {
    let mut creation = unsafe { mem::zeroed::<FILETIME>() };
    let mut exit = unsafe { mem::zeroed::<FILETIME>() };
    let mut kernel = unsafe { mem::zeroed::<FILETIME>() };
    let mut user = unsafe { mem::zeroed::<FILETIME>() };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
    Some((ticks / 10_000).saturating_sub(FILETIME_EPOCH_MS))
}

fn read_exe(handle: HANDLE) -> Option<String> {
    let mut buffer = vec![0u16; 4096];
    let mut size = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    (ok != 0).then(|| String::from_utf16_lossy(&buffer[..size as usize]))
}

/// One Toolhelp walk for the whole refresh: every pid's parent, so the
/// agent chain is walkable and a member's root is identifiable without a
/// machine-wide snapshot per member.
fn process_parents() -> Option<HashMap<u32, u32>> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut parents = HashMap::new();
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    Some(parents)
}

/// Both families' listener tables, read through the kernel's owner-pid API.
fn listener_ports() -> Option<Vec<(u16, u32)>> {
    let mut ports = Vec::new();
    for family in [u32::from(AF_INET), u32::from(AF_INET6)] {
        ports.extend(parse_owner_pid_table(&fetch_tcp_table(family)?, family));
    }
    Some(ports)
}

/// The raw table bytes for one address family, or `None` when the API
/// itself fails. A family with no data on this machine is an empty table,
/// not a failure.
fn fetch_tcp_table(family: u32) -> Option<Vec<u8>> {
    let mut size: u32 = 0;
    unsafe {
        GetExtendedTcpTable(
            ptr::null_mut(),
            &mut size,
            0,
            family,
            TCP_TABLE_OWNER_PID_LISTENER,
            0,
        )
    };
    if size == 0 {
        return Some(Vec::new());
    }
    let mut buffer = vec![0u8; size as usize];
    for attempt in 0..2 {
        // The call returns its own error code (`NO_ERROR` is 0); last-error
        // is not set by it and would only read some earlier failure.
        let status = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        match status {
            0 => {
                buffer.truncate(size as usize);
                return Some(buffer);
            }
            ERROR_NO_DATA => return Some(Vec::new()),
            // The size the first call asked for can grow between calls; retry once.
            ERROR_INSUFFICIENT_BUFFER if attempt == 0 => buffer.resize(size as usize, 0),
            _ => return None,
        }
    }
    None
}

/// One family's rows out of the raw table: the claimed row count is
/// bounds-checked against the bytes actually returned, so a kernel that
/// reports more rows than it wrote can never drive a read past the buffer.
fn parse_owner_pid_table(bytes: &[u8], family: u32) -> Vec<(u16, u32)> {
    if family == u32::from(AF_INET) {
        parse_v4_table(bytes)
    } else {
        parse_v6_table(bytes)
    }
}

fn claimed_rows(bytes: &[u8], stride: usize) -> usize {
    const HEADER: usize = mem::size_of::<u32>();
    if bytes.len() < HEADER {
        return 0;
    }
    // The buffer comes from the kernel on this little-endian platform.
    let claimed = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let available = bytes.len().saturating_sub(HEADER) / stride;
    claimed.min(available)
}

fn port_and_pid(local_port: u32, owning_pid: u32) -> Option<(u16, u32)> {
    (owning_pid != 0).then(|| (u16::from_be(local_port as u16), owning_pid))
}

fn parse_v4_table(bytes: &[u8]) -> Vec<(u16, u32)> {
    let stride = mem::size_of::<MIB_TCPROW_OWNER_PID>();
    let rows = claimed_rows(bytes, stride);
    let mut ports = Vec::new();
    for index in 0..rows {
        let offset = mem::size_of::<u32>() + index * stride;
        // SAFETY: `offset + stride` stays inside `bytes` — `claimed_rows`
        // divided by exactly this stride after the header.
        let row = unsafe { &*(bytes.as_ptr().add(offset) as *const MIB_TCPROW_OWNER_PID) };
        if let Some(found) = port_and_pid(row.dwLocalPort, row.dwOwningPid) {
            ports.push(found);
        }
    }
    ports
}

fn parse_v6_table(bytes: &[u8]) -> Vec<(u16, u32)> {
    let stride = mem::size_of::<MIB_TCP6ROW_OWNER_PID>();
    let rows = claimed_rows(bytes, stride);
    let mut ports = Vec::new();
    for index in 0..rows {
        let offset = mem::size_of::<u32>() + index * stride;
        // SAFETY: same bound as the v4 walk, against this family's own row
        // size — the 56-byte v6 row is never walked with the 24-byte stride.
        let row = unsafe { &*(bytes.as_ptr().add(offset) as *const MIB_TCP6ROW_OWNER_PID) };
        if let Some(found) = port_and_pid(row.dwLocalPort, row.dwOwningPid) {
            ports.push(found);
        }
    }
    ports
}

#[cfg(test)]
#[path = "process_probe_windows_tests.rs"]
mod tests;
