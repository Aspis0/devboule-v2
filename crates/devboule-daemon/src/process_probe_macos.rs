//! The macOS queries behind the process index: members, parent pids and the
//! rendered command line come from bounded `ps` calls, listener ports from a
//! bounded `lsof` — both killed at a hard deadline with capped output, the
//! way the peer listener's own helper is bounded.
//!
//! Two honest limits live here: `ps` prints a rendered command line, so argv
//! arrives whitespace-split (quoting is already lost in the display), and the
//! creation time is `lstart` parsed against the current timezone — two reads
//! that straddle a DST change or a locale `ps` cannot print in the English
//! form compare unequal, which replaces the entry rather than matching it
//! (the safe direction: a false mismatch never answers as the old process).

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::ProcessIdentity;
use crate::process_tree::JobObject;

pub(crate) const PROOF_KIND: &str = "process_group";

/// One helper's hard budget and output cap. A wedged `ps`/`lsof` is killed
/// with the budget, not left to stall the tool call.
const HELPER_BUDGET: Duration = Duration::from_secs(5);
const HELPER_POLL: Duration = Duration::from_millis(10);
const HELPER_OUTPUT_CAP: u64 = 256 * 1024;

pub(crate) fn members(job: &JobObject) -> Vec<u32> {
    let Some(group) = job.group_id() else {
        return Vec::new();
    };
    let Some(output) = run_bounded("ps", &["-axo", "pgid=,pid="]) else {
        return Vec::new();
    };
    let mut members = Vec::new();
    for line in output.lines() {
        let mut fields = line.split_whitespace();
        let (Some(pgid), Some(pid)) = (fields.next(), fields.next()) else {
            continue;
        };
        if pgid.parse::<u32>().is_ok_and(|parsed| parsed == group) {
            if let Ok(pid) = pid.parse::<u32>() {
                members.push(pid);
            }
        }
    }
    members
}

pub(crate) fn identity(pid: u32) -> Option<ProcessIdentity> {
    let pid_text = pid.to_string();
    let row = run_bounded("ps", &["-o", "pid=,ppid=,command=", "-p", &pid_text])?;
    let mut fields = row.split_whitespace();
    let _own_pid = fields.next()?;
    let ppid = fields.next()?.parse::<u32>().ok()?;
    let command: Vec<String> = fields.map(str::to_string).collect();
    let exe = command.first().cloned();

    let stamp = run_bounded("ps", &["-o", "lstart=", "-p", &pid_text])?;
    let started_at_ms = parse_lstart_ms(&stamp.trim())?;
    Some(ProcessIdentity {
        started_at_ms,
        ppid,
        exe,
        argv: command,
    })
}

pub(crate) fn listening_ports() -> Vec<(u16, u32)> {
    let Some(output) = run_bounded("lsof", &["-nP", "-iTCP", "-sTCP:LISTEN", "-F"]) else {
        return Vec::new();
    };
    let mut ports = Vec::new();
    let mut owner: Option<u32> = None;
    for line in output.lines() {
        if let Some(pid) = line.strip_prefix('p') {
            owner = pid.trim().parse().ok();
            continue;
        }
        let Some(name) = line.strip_prefix('n') else {
            continue;
        };
        let Some((_, port)) = name.rsplit_once(':') else {
            continue;
        };
        let digits: String = port
            .chars()
            .take_while(|cell| cell.is_ascii_digit())
            .collect();
        if let (Some(port), Some(owner)) = (digits.parse::<u16>().ok(), owner) {
            ports.push((port, owner));
        }
    }
    ports
}

/// `lstart` (`Wed Oct  5 10:00:00 2026`, ctime-shaped) as unix milliseconds:
/// the wall clock read in the *current* timezone offset. Two reads inside one
/// timezone regime agree exactly; a read that straddles a DST change does
/// not, which replaces the entry rather than matching it.
fn parse_lstart_ms(stamp: &str) -> Option<u64> {
    let mut fields = stamp.split_whitespace();
    let _weekday = fields.next()?;
    let month = match fields.next()? {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    };
    let day: u64 = fields.next()?.parse().ok()?;
    let mut clock = fields.next()?.split(':');
    let hour: u64 = clock.next()?.parse().ok()?;
    let minute: u64 = clock.next()?.parse().ok()?;
    let second: u64 = clock.next()?.parse().ok()?;
    let year: i64 = fields.next()?.parse().ok()?;
    let as_if_utc =
        days_from_civil(year, month, day) * 86_400 + (hour * 3_600 + minute * 60 + second) as i64;
    let epoch = as_if_utc - timezone_offset_seconds();
    u64::try_from(epoch).ok().map(|seconds| seconds * 1_000)
}

/// Howard Hinnant's days-from-civil (1970 epoch day for a proleptic
/// Gregorian date) — calendar arithmetic, no clock involved.
fn days_from_civil(year: i64, month: u64, day: u64) -> i64 {
    let year = year - (month <= 2) as i64;
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = (year - era * 400) as u64;
    let month_for_year_day = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_for_year_day + 2 * day + 5) / 6;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era as i64 - 719_468
}

/// The offset, in seconds, between the local wall clock and UTC *right now*:
/// `lstart` is printed in local time, and this is the only clock reading the
/// parse needs to turn it into an epoch. A DST change between two reads
/// changes the offset, and the entry is replaced instead of matched.
fn timezone_offset_seconds() -> i64 {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut local: libc::tm = std::mem::zeroed();
        let mut utc: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut local).is_null() || libc::gmtime_r(&now, &mut utc).is_null()
        {
            return 0;
        }
        let local_of_day = i64::from(local.tm_hour) * 3600
            + i64::from(local.tm_min) * 60
            + i64::from(local.tm_sec);
        let utc_of_day =
            i64::from(utc.tm_hour) * 3600 + i64::from(utc.tm_min) * 60 + i64::from(utc.tm_sec);
        // Wrap into [-12h, +12h]: the two times-of-day can differ across a
        // date boundary, and the difference is still the offset.
        let mut offset = local_of_day - utc_of_day;
        if offset > 43_200 {
            offset -= 86_400;
        } else if offset < -43_200 {
            offset += 86_400;
        }
        offset
    }
}

/// One bounded helper run: spawn, poll to the deadline (killing our own child
/// on the way out), read capped output. `None` means the helper itself could
/// not run — the caller then reports no members, never a guess.
fn run_bounded(program: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + HELPER_BUDGET;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(HELPER_POLL.min(remaining));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let stdout = child.stdout.take()?;
    let mut output = Vec::new();
    stdout
        .take(HELPER_OUTPUT_CAP + 1)
        .read_to_end(&mut output)
        .ok()?;
    if output.len() as u64 > HELPER_OUTPUT_CAP {
        return None;
    }
    Some(String::from_utf8_lossy(&output).into_owned())
}
