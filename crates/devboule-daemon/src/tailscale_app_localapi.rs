//! The macOS app-variant LocalAPI: find the app's localhost port and token,
//! then speak the same LocalAPI HTTP over that connection with basic auth.
//!
//! Discovery and the auth scheme are translated from Tailscale's `safesocket`
//! (`safesocket/safesocket_darwin.go`, commit `d69bf268`, BSD-3-Clause,
//! Copyright Tailscale Inc. & Contributors — see THIRD_PARTY.md). The token
//! is a credential: it is sent to 127.0.0.1 only and never appears in a
//! message, a cache key or a log line.

use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use super::{exchange_stream, find_header_end, LocalApiError};

/// Where the Standalone (macsys) variant leaves its port and token.
const SHARED_DIR: &str = "/Library/Tailscale";
/// The open file `lsof` shows for the App Store variant's daemon.
const LSOF_MARKER: &str = ".tailscale.ipn.macos/sameuserproof-";
/// The shared files outlive the daemon that wrote them, so a discovered port
/// is dialled once before it is offered — the Go source's stale check, which
/// must be 127.0.0.1 and not `localhost` (tailscale#7851).
const STALE_CHECK: Duration = Duration::from_secs(1);

pub(super) fn exchange(request: &[u8], deadline: Instant) -> Result<Vec<u8>, LocalApiError> {
    let (port, token) = discover(SHARED_DIR)?;
    let request = with_basic_auth(request, &token)?;
    exchange_tcp(port, &request, deadline)
}

/// Port and token, file evidence first — a plain read beats a subprocess —
/// then the App Store variant's `lsof`, run with the same flags the
/// Tailscale CLI uses (our uid, IPNExtension's own process). Both file
/// spellings are checked before `lsof`: the `ipnport` symlink with the token
/// in a separate file, then the token carried in the filename itself.
fn discover(dir: &Path) -> Result<(u16, String), LocalApiError> {
    if let Some(found) = read_macsys_files(dir) {
        return Ok(found);
    }
    if let Some(found) = read_filename_token_file(dir) {
        return Ok(found);
    }
    if let Some(found) = read_lsof() {
        return Ok(found);
    }
    Err(LocalApiError::Absent(format!(
        "the macOS app LocalAPI was not found: no usable port and token in {} (the ipnport \
         link or a sameuserproof file) and no {LSOF_MARKER}… file open by IPNExtension for \
         our uid",
        dir.display()
    )))
}

/// `/Library/Tailscale/ipnport` is a symlink to the localhost port, and
/// `sameuserproof-<port>` holds the token.
fn read_macsys_files(dir: &Path) -> Option<(u16, String)> {
    let target = std::fs::read_link(dir.join("ipnport")).ok()?;
    let port_text = target.to_str()?;
    // Digits only — the same text also names the file the token sits in.
    let port: u16 = port_text.parse().ok()?;
    let token = std::fs::read_to_string(dir.join(format!("sameuserproof-{port_text}"))).ok()?;
    let token = token.trim();
    if token.is_empty() || !stale_check(port) {
        return None;
    }
    Some((port, token.to_string()))
}

/// The other spelling: `sameuserproof-<port>-<token>` as one filename, the
/// form the Tailscale CLI scans a shared directory for.
fn read_filename_token_file(dir: &Path) -> Option<(u16, String)> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(rest) = name.strip_prefix("sameuserproof-") else {
            continue;
        };
        let Some((port_text, token)) = rest.split_once('-') else {
            continue;
        };
        let Ok(port) = port_text.parse::<u16>() else {
            continue;
        };
        if token.is_empty() || !stale_check(port) {
            continue;
        }
        return Some((port, token.to_string()));
    }
    None
}

fn stale_check(port: u16) -> bool {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    TcpStream::connect_timeout(&address, STALE_CHECK).is_ok()
}

fn read_lsof() -> Option<(u16, String)> {
    let uid = format!("-u{}", unsafe { libc::getuid() });
    let output = std::process::Command::new("lsof")
        .args(["-n", "-a", uid.as_str(), "-c", "IPNExtension", "-F"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_lsof_output(&output.stdout)
}

/// Scan `lsof -F` output for the open file whose name carries both halves
/// (`…sameuserproof-<port>-<token>`), the way the CLI reads it: the first
/// marker match decides, and a port that is not a port ends the search.
fn parse_lsof_output(output: &[u8]) -> Option<(u16, String)> {
    let text = String::from_utf8_lossy(output);
    for line in text.lines() {
        let Some(at) = line.find(LSOF_MARKER) else {
            continue;
        };
        let Some((port_text, token)) = line[at + LSOF_MARKER.len()..].split_once('-') else {
            continue;
        };
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
    let mut request = request.to_vec();
    let header_end = find_header_end(&request).ok_or_else(|| {
        LocalApiError::Protocol("the LocalAPI request has no header terminator".to_string())
    })?;
    request.insert_range(header_end, header.as_bytes());
    Ok(request)
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
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(LocalApiError::Timeout);
    }
    let _ = stream.set_read_timeout(Some(remaining));
    let _ = stream.set_write_timeout(Some(remaining));
    exchange_stream(stream, request)
}

#[cfg(test)]
#[path = "tailscale_app_localapi_tests.rs"]
mod tests;
