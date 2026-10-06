//! The macOS queries behind the process index: one bounded `ps` pair and one
//! bounded `lsof` per refresh feed every member, identity and port answer.
//!
//! Three honest limits live here: `ps` prints a rendered command line, so
//! argv arrives whitespace-split (quoting is already lost in the display);
//! the creation time is `lstart` at one-second resolution with the current
//! timezone offset applied — two reads that straddle a DST change or a
//! locale `ps` cannot print in the English form compare unequal, which
//! replaces the entry rather than matching it (the safe direction: a false
//! mismatch never answers as the old process); and a helper that cannot
//! start, times out or overflows its cap is an error the tool reports, never
//! an empty answer that would read as "no members".

use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{CreationStatus, ProcessIdentity};
use crate::process_tree::JobObject;

pub(crate) const PROOF_KIND: &str = "process_group";

/// One helper's hard budget and output cap. A wedged `ps`/`lsof` is killed
/// with the budget — and that kill is reported as an error, not as silence.
const HELPER_BUDGET: Duration = Duration::from_secs(5);
const HELPER_POLL: Duration = Duration::from_millis(10);
const HELPER_OUTPUT_CAP: u64 = 256 * 1024;

struct MacRow {
    pgid: u32,
    ppid: u32,
    command: String,
    started_at_ms: Option<u64>,
}

/// The platform probe: `begin_refresh` takes the three helper snapshots the
/// whole refresh reads from — two `ps` calls and one `lsof`, whatever the
/// membership size.
pub(crate) struct Probe {
    rows: HashMap<u32, MacRow>,
    ports: Vec<(u16, u32)>,
}

impl Probe {
    pub(crate) fn new() -> Self {
        Self {
            rows: HashMap::new(),
            ports: Vec::new(),
        }
    }

    pub(crate) fn begin_refresh(&mut self) -> Result<(), String> {
        let listings = run_bounded("ps", &["-axo", "pid=,ppid=,pgid=,command="])?;
        let stamps = run_bounded("ps", &["-axo", "pid=,lstart="])?;
        self.rows = parse_ps_rows(&listings);
        for line in stamps.lines() {
            let Some((pid, stamp)) = line.split_once(' ') else {
                continue;
            };
            if let (Ok(pid), Some(started_at_ms)) =
                (pid.parse::<u32>(), parse_lstart_ms(stamp.trim()))
            {
                if let Some(row) = self.rows.get_mut(&pid) {
                    row.started_at_ms = Some(started_at_ms);
                }
            }
        }
        self.ports = parse_lsof_ports(&run_bounded(
            "lsof",
            &["-nP", "-iTCP", "-sTCP:LISTEN", "-F"],
        )?);
        Ok(())
    }

    pub(crate) fn members(&self, job: &JobObject) -> Vec<u32> {
        let Some(group) = job.group_id() else {
            return Vec::new();
        };
        self.rows
            .iter()
            .filter(|(_, row)| row.pgid == group)
            .map(|(pid, _)| *pid)
            .collect()
    }

    pub(crate) fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
        // A row without a readable start time is a member the OS would not
        // vouch for: no identity, never a target.
        let row = self.rows.get(&pid)?;
        let started_at_ms = row.started_at_ms?;
        let command: Vec<String> = row.command.split_whitespace().map(str::to_string).collect();
        Some(ProcessIdentity {
            started_at_ms,
            ppid: row.ppid,
            exe: command.first().cloned(),
            argv: command,
        })
    }

    pub(crate) fn listening_ports(&self) -> Vec<(u16, u32)> {
        self.ports.clone()
    }
}

/// The creation-time read the terminate-time identity check uses.
pub(crate) fn creation_status(pid: u32) -> CreationStatus {
    let output = match run_bounded("ps", &["-o", "lstart=", "-p", &pid.to_string()]) {
        Ok(output) => output,
        Err(_) => return CreationStatus::Unverified,
    };
    if output.trim().is_empty() {
        return CreationStatus::Gone;
    }
    parse_lstart_ms(output.trim())
        .map(CreationStatus::At)
        .unwrap_or(CreationStatus::Unverified)
}

/// One `ps -axo` listing into the row table: pid, parent, group and the
/// rendered command line. A line without three numeric fields cannot prove
/// anything and is dropped; a row whose start time never arrives keeps
/// `started_at_ms = None`, which reads as unproven rather than invisible.
fn parse_ps_rows(listing: &str) -> HashMap<u32, MacRow> {
    let mut rows = HashMap::new();
    for line in listing.lines() {
        let mut fields = line.splitn(4, ' ');
        let (Some(pid), Some(ppid), Some(pgid)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (Ok(pid), Ok(ppid), Ok(pgid)) = (
            pid.trim().parse::<u32>(),
            ppid.trim().parse::<u32>(),
            pgid.trim().parse::<u32>(),
        ) else {
            continue;
        };
        let command = fields.next().unwrap_or_default().to_string();
        rows.insert(
            pid,
            MacRow {
                pgid,
                ppid,
                command,
                started_at_ms: None,
            },
        );
    }
    rows
}

/// `lsof -F` records into (port, pid) pairs: a pid line sets the owner of
/// every name line that follows it.
fn parse_lsof_ports(output: &str) -> Vec<(u16, u32)> {
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

/// One bounded helper run: spawn, read stdout on its own thread while the
/// deadline runs (a child that fills the pipe blocks on the reader, never on
/// us), kill on overrun. Every failure — start, timeout, cap — is an error
/// the caller must surface; only a successful read with empty output means
/// "nothing to report".
fn run_bounded(program: &str, args: &[&str]) -> Result<String, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("{program} could not start: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{program} stdout is unavailable"))?;
    let reader = std::thread::spawn(move || {
        let mut buffered = Vec::new();
        let _ = stdout
            .take(HELPER_OUTPUT_CAP + 1)
            .read_to_end(&mut buffered);
        buffered
    });
    let deadline = Instant::now() + HELPER_BUDGET;
    let wait_result = loop {
        match child.try_wait() {
            Ok(Some(_)) => break Ok(()),
            Ok(None) if Instant::now() < deadline => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                std::thread::sleep(HELPER_POLL.min(remaining));
            }
            Ok(None) | Err(_) => {
                break Err(format!(
                    "{program} did not answer within its {}ms budget",
                    HELPER_BUDGET.as_millis()
                ));
            }
        }
    };
    if wait_result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return wait_result;
    }
    let output = reader
        .join()
        .map_err(|_| format!("{program} output thread panicked"))?;
    if output.len() as u64 > HELPER_OUTPUT_CAP {
        return Err(format!("{program} output exceeded its cap"));
    }
    Ok(String::from_utf8_lossy(&output).into_owned())
}
