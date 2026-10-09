//! The log line for a client hello whose owner differs from the peer the
//! connection proved. On a paired link the dialing daemon's hello says
//! `client: "daemon"` while the responder stores that peer as a client, so the
//! mismatch repeats on every dial and is expected. The line names both role
//! tags, which are roles and not identities, and redacts each user.

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
        hello.client,
        redact(&peer.user),
        peer.client,
    ))
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
