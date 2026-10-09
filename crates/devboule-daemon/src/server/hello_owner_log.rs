//! The log line for a client hello whose owner differs from the peer the
//! connection proved. On a paired link the dialing daemon's hello says
//! `client: "daemon"` while the responder stores that peer as a client, so the
//! mismatch repeats on every dial and is expected. The line names both roles by
//! a fixed word and redacts each user; the client tag itself is never printed.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

use devboule_protocol::OwnerId;

use crate::device_identity::redact;

static LOGGED_PEERS: LazyLock<Mutex<HashSet<String>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

/// Write the mismatch for `peer` once per process, however often it dials.
pub(super) fn log_mismatch_once(hello: &OwnerId, peer: &OwnerId) {
    let line = {
        let mut seen = LOGGED_PEERS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        first_mismatch_line(&mut seen, hello, peer)
    };
    if let Some(line) = line {
        eprintln!("{line}");
    }
}

/// The line for a peer not in `seen` yet, and `None` for a repeat from it.
fn first_mismatch_line(
    seen: &mut HashSet<String>,
    hello: &OwnerId,
    peer: &OwnerId,
) -> Option<String> {
    if !seen.insert(redact(&peer.user)) {
        return None;
    }
    Some(format!(
        "client hello owner label {} (role {}) did not match the connection peer {} (role {})",
        redact(&hello.user),
        role_tag(&hello.client),
        redact(&peer.user),
        role_tag(&peer.client),
    ))
}

/// The fixed words a log line may carry for a role. The client token is
/// peer-supplied on the hello and is never printed as it arrived.
fn role_tag(client: &str) -> &'static str {
    match client {
        "client" => "client",
        "daemon" => "daemon",
        other if other.starts_with("process-") => "process",
        _ => "unrecognised",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(user: &str, client: &str) -> OwnerId {
        OwnerId::new(user, client).expect("valid owner")
    }

    #[test]
    fn the_line_names_both_role_tags_and_no_raw_identity() {
        let mut seen = HashSet::new();
        let hello = owner("peer_dev-1", "daemon");
        let peer = owner("peer_dev-1", "client");
        let line = first_mismatch_line(&mut seen, &hello, &peer).expect("first sighting");
        assert!(line.contains("(role daemon)"), "{line}");
        assert!(line.contains("(role client)"), "{line}");
        assert!(!line.contains("peer_dev-1"), "{line}");
    }

    #[test]
    fn peer_supplied_role_text_is_never_printed() {
        // The hello's client token arrives on the wire unvalidated: a newline
        // or an escape in it must not reach the log.
        let mut seen = HashSet::new();
        let hostile = OwnerId {
            user: "peer_dev-1".to_string(),
            client: "daemon\n[devboule] forged line \u{1b}[2J".to_string(),
        };
        let peer = owner("peer_dev-1", "client");
        let line = first_mismatch_line(&mut seen, &hostile, &peer).expect("first sighting");
        assert!(!line.contains('\n'), "{line:?}");
        assert!(!line.contains("forged"), "{line:?}");
        assert!(!line.contains('\u{1b}'), "{line:?}");
        assert!(line.contains("(role unrecognised)"), "{line}");
    }

    #[test]
    fn a_local_process_is_named_by_its_role_not_its_pid() {
        let mut seen = HashSet::new();
        let hello = owner("peer_dev-1", "daemon");
        let local = owner("S-1-5-21-1", "process-4242");
        let line = first_mismatch_line(&mut seen, &hello, &local).expect("first sighting");
        assert!(line.contains("(role process)"), "{line}");
        assert!(!line.contains("4242"), "{line}");
    }

    #[test]
    fn a_repeat_from_the_same_peer_is_not_written_again() {
        let mut seen = HashSet::new();
        let hello = owner("peer_dev-1", "daemon");
        let peer = owner("peer_dev-1", "client");
        assert!(first_mismatch_line(&mut seen, &hello, &peer).is_some());
        assert_eq!(first_mismatch_line(&mut seen, &hello, &peer), None);
    }

    #[test]
    fn another_peer_is_written_too() {
        let mut seen = HashSet::new();
        let hello = owner("peer_dev-1", "daemon");
        let first = owner("peer_dev-1", "client");
        let second = owner("peer_dev-2", "client");
        assert!(first_mismatch_line(&mut seen, &hello, &first).is_some());
        assert!(first_mismatch_line(&mut seen, &hello, &second).is_some());
    }
}
