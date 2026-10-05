//! The Windows queries behind the process index: members come from the
//! session's Job Object, identity from an opened handle plus one Toolhelp
//! snapshot per call, listener ports from the kernel's owner-PID TCP tables.
//!
//! Windows cannot read another process's command line through a documented
//! API, so `argv` is empty here and an entry carries its exe only — the
//! macOS half is the one that can show argv, and the report records this.

use std::mem;
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, FILETIME, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCPROW_OWNER_PID, TCP_TABLE_OWNER_PID_LISTENER,
};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::ProcessIdentity;

pub(crate) const PROOF_KIND: &str = "job_member";

pub(crate) fn members(job: &crate::process_tree::JobObject) -> Vec<u32> {
    // A failed query means the job handle is gone (the session ended), and a
    // dead job has no members to claim.
    job.pids().unwrap_or_default()
}

pub(crate) fn identity(pid: u32) -> Option<ProcessIdentity> {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let identity = read_identity(handle, pid);
    unsafe { CloseHandle(handle) };
    identity
}

fn read_identity(handle: HANDLE, pid: u32) -> Option<ProcessIdentity> {
    let mut creation = unsafe { mem::zeroed::<FILETIME>() };
    let mut exit = unsafe { mem::zeroed::<FILETIME>() };
    let mut kernel = unsafe { mem::zeroed::<FILETIME>() };
    let mut user = unsafe { mem::zeroed::<FILETIME>() };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    // FILETIME is 100 ns ticks since 1601; shift it to unix milliseconds so a
    // tool result's `elapsed` means the same thing on both platforms.
    let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
    const FILETIME_EPOCH_MS: u64 = 116_444_736_000_000;
    let started_at_ms = (ticks / 10_000).saturating_sub(FILETIME_EPOCH_MS);

    let mut buffer = vec![0u16; 4096];
    let mut size = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    let exe = (ok != 0).then(|| String::from_utf16_lossy(&buffer[..size as usize]));

    let ppid = parent_pid(pid).unwrap_or(0);
    Some(ProcessIdentity {
        started_at_ms,
        ppid,
        exe,
        argv: Vec::new(),
    })
}

/// One Toolhelp snapshot to answer one parent-pid question: the root of a
/// session's tree is the member whose parent is the daemon itself.
fn parent_pid(pid: u32) -> Option<u32> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return None;
    }
    let mut entry: PROCESSENTRY32W = unsafe { mem::zeroed() };
    entry.dwSize = mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snapshot, &mut entry) };
    while ok != 0 {
        if entry.th32ProcessID == pid {
            let ppid = entry.th32ParentProcessID;
            unsafe { CloseHandle(snapshot) };
            return Some(ppid);
        }
        ok = unsafe { Process32NextW(snapshot, &mut entry) };
    }
    unsafe { CloseHandle(snapshot) };
    None
}

pub(crate) fn listening_ports() -> Vec<(u16, u32)> {
    let mut ports = Vec::new();
    for family in [u32::from(AF_INET), u32::from(AF_INET6)] {
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
        let mut buffer = vec![0u8; size as usize];
        if size == 0 {
            continue;
        }
        let ok = unsafe {
            GetExtendedTcpTable(
                buffer.as_mut_ptr().cast(),
                &mut size,
                0,
                family,
                TCP_TABLE_OWNER_PID_LISTENER,
                0,
            )
        };
        if ok == 0 {
            let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
            if err != ERROR_INSUFFICIENT_BUFFER {
                continue;
            }
            // The size the first call asked for can grow between calls; retry once.
            buffer.resize(size as usize, 0);
            let retry = unsafe {
                GetExtendedTcpTable(
                    buffer.as_mut_ptr().cast(),
                    &mut size,
                    0,
                    family,
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                )
            };
            if retry == 0 {
                continue;
            }
        }
        let stride = mem::size_of::<MIB_TCPROW_OWNER_PID>();
        let header = mem::size_of::<u32>();
        let usable = (size as usize).saturating_sub(header);
        let rows = usable / stride;
        let base = buffer.as_ptr();
        for index in 0..rows {
            let row =
                unsafe { &*(base.add(header + index * stride) as *const MIB_TCPROW_OWNER_PID) };
            if row.dwOwningPid == 0 {
                continue;
            }
            ports.push((u16::from_be(row.dwLocalPort as u16), row.dwOwningPid));
        }
    }
    ports
}
