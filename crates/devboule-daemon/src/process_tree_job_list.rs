//! Reading a Job Object's process-id list. The kernel fills what fits and
//! reports how many processes are assigned; a list shorter than that is read
//! again with room for all of them, and refused when it stays short. A short
//! list read as complete would hide a member, and the member could be the
//! bridge between a target and the provider.

use std::io;
use std::mem;

/// Reads of a list the kernel keeps reporting short before the read is refused.
pub(crate) const JOB_LIST_ATTEMPTS: usize = 3;

/// The most members a job list may hold; a larger answer is refused, not allocated.
const MAX_JOB_MEMBERS: u32 = 16_384;

/// `NumberOfAssignedProcesses` and `NumberOfProcessIdsInList`, both `u32`.
const HEADER: usize = 2 * mem::size_of::<u32>();

/// One process id slot: the kernel stores each as a pointer-sized value.
const SLOT: usize = mem::size_of::<usize>();

/// Reads the job's id list through `query`, which fills the buffer it is given
/// with one answer. A list the kernel reports short is asked again with room
/// for every assigned process, at most `JOB_LIST_ATTEMPTS` times.
pub(crate) fn read_pid_list(
    mut query: impl FnMut(&mut Vec<u8>) -> io::Result<()>,
) -> io::Result<Vec<u32>> {
    let mut buffer = vec![0u8; 4096];
    for _ in 0..JOB_LIST_ATTEMPTS {
        query(&mut buffer)?;
        let (assigned, listed) = header(&buffer)?;
        if assigned > MAX_JOB_MEMBERS || listed > assigned {
            return Err(invalid("the job reports an implausible member count"));
        }
        if listed == assigned {
            return ids(&buffer, listed);
        }
        buffer.resize(HEADER + assigned as usize * SLOT, 0);
    }
    Err(invalid(
        "the job list stayed short of its assigned processes",
    ))
}

fn header(buffer: &[u8]) -> io::Result<(u32, u32)> {
    if buffer.len() < HEADER {
        return Err(invalid("the job list has no header"));
    }
    let assigned = u32::from_ne_bytes(buffer[0..4].try_into().expect("four bytes"));
    let listed = u32::from_ne_bytes(buffer[4..8].try_into().expect("four bytes"));
    Ok((assigned, listed))
}

fn ids(buffer: &[u8], listed: u32) -> io::Result<Vec<u32>> {
    if buffer.len() < HEADER + listed as usize * SLOT {
        return Err(invalid("the job list is shorter than its own count"));
    }
    Ok((0..listed as usize)
        .map(|index| {
            let start = HEADER + index * SLOT;
            let raw = usize::from_ne_bytes(buffer[start..start + SLOT].try_into().expect("a slot"));
            raw as u32
        })
        .collect())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// The kernel's answer for a job with `assigned` members, of which only
    /// `ids` fit the buffer: the header, then one slot per listed id.
    fn answer(buffer: &mut Vec<u8>, assigned: u32, ids: &[u32]) {
        *buffer = vec![0u8; HEADER + ids.len() * SLOT];
        buffer[0..4].copy_from_slice(&assigned.to_ne_bytes());
        buffer[4..8].copy_from_slice(&(ids.len() as u32).to_ne_bytes());
        for (index, id) in ids.iter().enumerate() {
            let start = HEADER + index * SLOT;
            buffer[start..start + SLOT].copy_from_slice(&(*id as usize).to_ne_bytes());
        }
    }

    /// A short list is read again, into a buffer that holds every assigned
    /// process, and the complete answer is the one returned.
    #[test]
    fn a_short_job_list_is_read_again_until_it_is_whole() {
        let calls = Cell::new(0);
        let ids = read_pid_list(|buffer| {
            calls.set(calls.get() + 1);
            if calls.get() == 1 {
                answer(buffer, 3, &[1, 2]);
            } else {
                assert!(
                    buffer.len() >= HEADER + 3 * SLOT,
                    "the retry gets room for all three assigned"
                );
                answer(buffer, 3, &[1, 2, 3]);
            }
            Ok(())
        });
        assert_eq!(ids.expect("a whole list"), vec![1, 2, 3]);
        assert_eq!(calls.get(), 2);
    }

    /// A list that stays short after every attempt is refused: the plan built
    /// from it would be missing members it cannot name.
    #[test]
    fn a_job_list_that_stays_short_is_refused() {
        let calls = Cell::new(0);
        let ids = read_pid_list(|buffer| {
            calls.set(calls.get() + 1);
            answer(buffer, 3, &[1, 2]);
            Ok(())
        });
        assert!(ids.is_err(), "a short list must not pass for a whole one");
        assert_eq!(calls.get(), JOB_LIST_ATTEMPTS);
    }

    /// A whole list is read once and returned as it is.
    #[test]
    fn a_whole_job_list_is_read_once() {
        let calls = Cell::new(0);
        let ids = read_pid_list(|buffer| {
            calls.set(calls.get() + 1);
            answer(buffer, 2, &[7, 8]);
            Ok(())
        });
        assert_eq!(ids.expect("a whole list"), vec![7, 8]);
        assert_eq!(calls.get(), 1);
    }

    /// An answer claiming more processes than it could have assigned is
    /// refused, never allocated for.
    #[test]
    fn an_implausible_job_list_is_refused() {
        let ids = read_pid_list(|buffer| {
            answer(buffer, MAX_JOB_MEMBERS + 1, &[1]);
            Ok(())
        });
        assert!(ids.is_err());
    }
}
