//! The macOS app-variant LocalAPI: find the app's localhost port and token,
//! then speak the same LocalAPI HTTP over that connection with basic auth.
//!
//! Discovery and the auth scheme are translated from Tailscale's `safesocket`
//! (`safesocket/safesocket_darwin.go`, commit `d69bf268`, BSD-3-Clause,
//! Copyright Tailscale Inc. & Contributors — see THIRD_PARTY.md), in the
//! order the CLI itself tries them. The token is a credential: it is sent to
//! 127.0.0.1 only and never appears in a message, a cache key or a log line.

use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::{exchange_stream, find_header_end, LocalApiError};

/// Where the Standalone (macsys) variant leaves its port and token.
const SHARED_DIR: &str = "/Library/Tailscale";
/// The open file `lsof` shows for the App Store variant's daemon.
const LSOF_MARKER: &str = ".tailscale.ipn.macos/sameuserproof-";
/// The shared files outlive the daemon that wrote them, so a discovered port
/// is dialled once before it is offered — the Go source's stale check, which
/// must be 127.0.0.1 and not `localhost` (tailscale#7851), and never longer
/// than the caller's remaining budget.
const STALE_CHECK: Duration = Duration::from_secs(1);
/// `lsof` gets the caller's remaining budget and not a millisecond more; its
/// output is read only up to this cap, because the parse needs one field.
const LSOF_OUTPUT_CAP: u64 = 64 * 1024;
/// Poll period while waiting for our own `lsof` child to exit.
const LSOF_POLL: Duration = Duration::from_millis(10);

pub(super) fn exchange(request: &[u8], deadline: Instant) -> Result<Vec<u8>, LocalApiError> {
    let (port, token) = discover(Path::new(SHARED_DIR), deadline)?;
    let request = with_basic_auth(request, &token)?;
    exchange_tcp(port, &request, deadline)
}

/// Port and token in the CLI's order: App Store (`lsof`) first, then the
/// Macsys files (`portAndTokenFromSameUserProof` at the cited commit). One
/// refusal names both places when neither answers.
fn discover(dir: &Path, deadline: Instant) -> Result<(u16, String), LocalApiError> {
    if let Some(found) = read_lsof(deadline) {
        return Ok(found);
    }
    if let Some(found) = read_macsys_files(dir, deadline) {
        return Ok(found);
    }
    Err(LocalApiError::Absent(format!(
        "the macOS app LocalAPI was not found: no {}… file open by IPNExtension for our uid, \
         and no usable port and token in {} (the ipnport link and its sameuserproof file)",
        LSOF_MARKER,
        dir.display()
    )))
}

/// `readMacsysSameUserProof`: the symlink names the port, the file holds the
/// token, and both are allowed to be stale — so the port is dialled once,
/// within the caller's budget, before it is offered.
fn read_macsys_files(dir: &Path, deadline: Instant) -> Option<(u16, String)> {
    let target = std::fs::read_link(dir.join("ipnport")).ok()?;
    let port_text = target.to_str()?;
    // Digits only — the same text also names the file the token sits in.
    let port: u16 = port_text.parse().ok()?;
    let token = std::fs::read_to_string(dir.join(format!("sameuserproof-{port_text}"))).ok()?;
    let token = token.trim();
    if token.is_empty() || !stale_check(port, deadline) {
        return None;
    }
    Some((port, token.to_string()))
}

fn stale_check(port: u16, deadline: Instant) -> bool {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return false;
    }
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, remaining.min(STALE_CHECK)).is_ok()
}

/// `readMacosSameUserProof`'s `lsof`, bounded like everything else on this
/// path: our own child, killed by its own pid when the budget runs out, its
/// output read only after it exits and only up to the cap.
fn read_lsof(deadline: Instant) -> Option<(u16, String)> {
    if deadline.saturating_duration_since(Instant::now()).is_zero() {
        return None;
    }
    let uid = format!("-u{}", unsafe { libc::getuid() });
    let mut child = Command::new("lsof")
        .args(["-n", "-a", uid.as_str(), "-c", "IPNExtension", "-F"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    // `lsof` exits non-zero when it found nothing.
                    return None;
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(LSOF_POLL),
            Ok(None) | Err(_) => {
                // OUR child: kill and reap it, so a wedged lsof cannot
                // outlive this call or leave a zombie behind.
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let stdout = child.stdout.take()?;
    let mut output = Vec::new();
    stdout
        .take(LSOF_OUTPUT_CAP + 1)
        .read_to_end(&mut output)
        .ok()?;
    if output.len() as u64 > LSOF_OUTPUT_CAP {
        return None;
    }
    parse_lsof_output(&output)
}

/// `lsof -F` read as records: `p` starts a process, `c` names its command
/// and must be the one the filter asked for, `f` starts a file, and only an
/// `n` filename line inside such a file record may carry the marker — a
/// continuation line from a crafted filename carries no field prefix and is
/// ignored.
fn parse_lsof_output(output: &[u8]) -> Option<(u16, String)> {
    let text = String::from_utf8_lossy(output);
    let mut command_matches = false;
    let mut file_record = false;
    for line in text.lines() {
        if line.starts_with('p') {
            command_matches = false;
            file_record = false;
            continue;
        }
        if let Some(command) = line.strip_prefix('c') {
            // `-c IPNExtension` is a prefix match, so accept the same shape.
            command_matches = command.starts_with("IPNExtension");
            file_record = false;
            continue;
        }
        if line.starts_with('f') {
            file_record = command_matches;
            continue;
        }
        if !file_record {
            continue;
        }
        let Some(name) = line.strip_prefix('n') else {
            continue;
        };
        let Some(at) = name.find(LSOF_MARKER) else {
            continue;
        };
        let Some((port_text, token)) = name[at + LSOF_MARKER.len()..].split_once('-') else {
            continue;
        };
        // The marker sat in a real filename field: a port that is not a port
        // ends the search, as the CLI's parse does.
        return port_text
            .parse::<u16>()
            .ok()
            .map(|port| (port, token.to_string()));
    }
    None
}

/// The auth the LocalAPI server checks: HTTP basic with an empty username —
/// `ipn/localapi` reads only the password (`_, pass, ok := r.BasicAuth()`).
fn with_basic_auth(request: &[u8], token: &str) -> Result<Vec<u8>, LocalApiError> {
    use base64::Engine;

    let header = format!(
        "Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!(":{token}"))
    );
    let header_end = find_header_end(request).ok_or_else(|| {
        LocalApiError::Protocol("the LocalAPI request has no header terminator".to_string())
    })?;
    let mut authorized = Vec::with_capacity(request.len() + header.len());
    authorized.extend_from_slice(&request[..header_end]);
    authorized.extend_from_slice(header.as_bytes());
    authorized.extend_from_slice(&request[header_end..]);
    Ok(authorized)
}

fn exchange_tcp(port: u16, request: &[u8], deadline: Instant) -> Result<Vec<u8>, LocalApiError> {
    // 127.0.0.1 by construction: this function takes a port, never a host,
    // so the token cannot be sent anywhere else.
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(LocalApiError::Timeout);
    }
    let stream = TcpStream::connect_timeout(&address, remaining).map_err(|error| {
        LocalApiError::Transport(format!("the app LocalAPI at {address}: {error}"))
    })?;
    exchange_stream(stream, request, deadline)
}

#[cfg(test)]
#[path = "tailscale_app_localapi_tests.rs"]
mod tests;
