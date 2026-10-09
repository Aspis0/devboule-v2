//! Devices domain — pass-3a split of `server.rs`: the device commands
//! (`dispatch_devices`) and the pairing/address helpers behind them.

use super::*;

/// The device commands. Everything here is a local act except `DevicesList`,
/// which is projected for whoever asks: a local client sees the full rows, a
/// remote peer sees a subset (design §8b A13, muse M4).
pub(super) fn dispatch_devices(
    state: &Arc<ServerState>,
    conn: &Arc<ConnHandle>,
    request: ClientMessage,
    _passed: &GatePassed,
) -> DaemonMessage {
    let reply = match request {
        ClientMessage::DevicesList { id } => match devices_reply(state, &conn.conn_peer) {
            Ok(reply) => DaemonMessage::Devices {
                id,
                self_info: reply.self_info,
                peers: reply.peers,
                pending: reply.pending,
            },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::PairingStart { id } => {
            // The address shown is where *this* device can be reached.
            match pairing_address(state) {
                Err(error) => DaemonMessage::Error(error.with_id(id)),
                Ok(address) => match state.pairing().start() {
                    Ok((code, expires_at)) => DaemonMessage::PairingCode {
                        id,
                        code,
                        expires_at,
                        address,
                    },
                    // Only the OS entropy source can fail here, and refusing
                    // is the only safe answer.
                    Err(error) => DaemonMessage::Error(
                        WireError::new(ErrorCode::Internal, error.to_string()).with_id(id),
                    ),
                },
            }
        }
        ClientMessage::PairingComplete { id, address, code } => {
            // The pairing service takes this device's transport from its own
            // state, so the binding check on both the immediate path and the
            // deferred answer thread see the same one.
            // The message never contains the code: `PairingError` renders
            // only reasons, and `PairingSecret`'s `Debug` is redacted. A
            // pairing always parks on the code-displaying device, so this
            // reply is always the pending one.
            match state.pairing().complete(state, &address, &code) {
                Ok(crate::pairing::PairingOutcome::Pending(peer)) => {
                    DaemonMessage::PairingPending { id, peer }
                }
                Err(error) => DaemonMessage::Error(
                    WireError::new(ErrorCode::InvalidRequest, error.to_string()).with_id(id),
                ),
            }
        }
        ClientMessage::PairingConfirm {
            id,
            device_id,
            accept,
        } => match state.pairing().confirm(state, &device_id, accept) {
            Ok(crate::pairing::ConfirmOutcome::Accepted(peer)) => {
                DaemonMessage::PeerUpdated { id, peer: *peer }
            }
            // A decline is a completed act, not an error: the panel must not
            // render it as a failure.
            Ok(crate::pairing::ConfirmOutcome::Declined) => {
                DaemonMessage::PairingDeclined { id, device_id }
            }
            Err(error) => DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, error.to_string()).with_id(id),
            ),
        },
        ClientMessage::PeerRevoke { id, device_id } => {
            match state.peer_revoke(&device_id, unix_millis() as i64) {
                Ok(PeerMutation::Updated) => {
                    // Revocation closes live connections under the same lock
                    // that recorded it, so at most one already-decoded frame
                    // is processed afterwards (design §8 R8).
                    let closed = state.revoke_peer_connections(&device_id);
                    let _ = closed;
                    match state.peer_get(&device_id) {
                        Ok(Some(record)) => DaemonMessage::PeerUpdated {
                            id,
                            peer: crate::pairing::peer_row(state, &record),
                        },
                        _ => DaemonMessage::Ok { id },
                    }
                }
                // A row that is already revoked is a different fact from a row
                // that does not exist, and the panel shows this sentence
                // verbatim: a double click, or a second device's panel, must
                // not read it as a missing peer.
                Ok(PeerMutation::Revoked) => DaemonMessage::Error(
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "That device is already revoked. Pair it again to use it.",
                    )
                    .with_id(id),
                ),
                Ok(PeerMutation::NotFound) => DaemonMessage::Error(
                    WireError::new(ErrorCode::InvalidRequest, "No such peer to revoke.")
                        .with_id(id),
                ),
                Err(error) => {
                    DaemonMessage::Error(WireError::new(ErrorCode::Internal, error).with_id(id))
                }
            }
        }
        ClientMessage::PeerSetCaps {
            id,
            device_id,
            caps,
        } => match state.peer_get(&device_id) {
            Err(error) => {
                DaemonMessage::Error(WireError::new(ErrorCode::Internal, error).with_id(id))
            }
            Ok(None) => DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, "No such peer.").with_id(id),
            ),
            Ok(Some(_)) => {
                match crate::pairing::validate_caps(&caps) {
                    Err(message) => DaemonMessage::Error(
                        WireError::new(ErrorCode::InvalidRequest, message).with_id(id),
                    ),
                    Ok(caps) => match state.peer_set_caps(&device_id, caps) {
                        Ok(PeerMutation::Updated) => {
                            // A capability change takes effect on the next
                            // connection: the live one holds the set it read at
                            // connect, and a device must not keep a capability
                            // this row no longer grants. The flag is what drops
                            // it, on that connection's own next turn.
                            state.revoke_peer_connections(&device_id);
                            match state.peer_get(&device_id) {
                                Ok(Some(refreshed)) => DaemonMessage::PeerUpdated {
                                    id,
                                    peer: crate::pairing::peer_row(state, &refreshed),
                                },
                                _ => DaemonMessage::Ok { id },
                            }
                        }
                        // A revoked device's capabilities cannot be rewritten
                        // (C8): the stored state would disagree with the
                        // panel's "Revoked" section, and the old array would
                        // silently revive if the row is re-paired.
                        Ok(PeerMutation::Revoked) => DaemonMessage::Error(
                            WireError::new(
                                ErrorCode::InvalidRequest,
                                "That device is revoked; its capabilities cannot be changed. \
                                 Pair it again to use it.",
                            )
                            .with_id(id),
                        ),
                        Ok(PeerMutation::NotFound) => DaemonMessage::Error(
                            WireError::new(ErrorCode::InvalidRequest, "No such peer.").with_id(id),
                        ),
                        Err(error) => DaemonMessage::Error(
                            WireError::new(ErrorCode::Internal, error).with_id(id),
                        ),
                    },
                }
            }
        },
        other => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            format!("unexpected device frame {other:?}"),
        )),
    };
    // The devices family is built with the v30 role projection so an old
    // decoder can parse it; a connection that negotiated the roleless dialect
    // never sees the word. This is the one place the projection leaves the
    // daemon, so no other reply path can leak it. A v30 connection is the only
    // one that keeps it, and it is also the only one whose decoder requires it.
    if conn.negotiated_protocol() <= LEGACY_DEVICES_DIALECT {
        reply
    } else {
        strip_projected_roles(reply)
    }
}

/// The last app/daemon dialect whose `devices`/pairing DTOs carried a required
/// `role` field. Everything at or below it is handed the projection; nothing
/// above it is.
const LEGACY_DEVICES_DIALECT: u32 = 30;

/// Drop the v30 role projection from one devices-family reply. The rows are
/// built once with the word a v30 decoder requires; this is where a v32
/// connection's copy loses it.
fn strip_projected_roles(reply: DaemonMessage) -> DaemonMessage {
    match reply {
        DaemonMessage::Devices {
            id,
            self_info,
            peers,
            pending,
        } => DaemonMessage::Devices {
            id,
            self_info,
            peers: peers.into_iter().map(roleless_peer).collect(),
            pending: pending.into_iter().map(roleless_pending).collect(),
        },
        DaemonMessage::PairingPending { id, peer } => DaemonMessage::PairingPending {
            id,
            peer: roleless_pending(peer),
        },
        DaemonMessage::PairingDone { id, peer } => DaemonMessage::PairingDone {
            id,
            peer: roleless_peer(peer),
        },
        DaemonMessage::PeerUpdated { id, peer } => DaemonMessage::PeerUpdated {
            id,
            peer: roleless_peer(peer),
        },
        other => other,
    }
}

fn roleless_peer(mut peer: devboule_protocol::PeerRow) -> devboule_protocol::PeerRow {
    peer.role = None;
    peer
}

fn roleless_pending(
    mut peer: devboule_protocol::PendingPairing,
) -> devboule_protocol::PendingPairing {
    peer.role = None;
    peer
}

struct DevicesReply {
    self_info: devboule_protocol::SelfInfo,
    peers: Vec<devboule_protocol::PeerRow>,
    pending: Vec<devboule_protocol::PendingPairing>,
}

/// Build the reply for the requesting scope.
///
/// A machine peer sees `{device_id, display_name, online}` per peer and
/// `{device_id, display_name, daemon_version, protocol_version}` for this
/// device: never an address, a binding, a public key, or the SID this device
/// paired it from. Its own Noise key is what it needs, and it already has it.
fn devices_reply(
    state: &Arc<ServerState>,
    conn_peer: &Option<ConnPeer>,
) -> Result<DevicesReply, WireError> {
    let records = state
        .peers()
        .map_err(|error| WireError::new(ErrorCode::Internal, error))?;
    let identity = state
        .device_identity()
        .as_ref()
        .map_err(|error| WireError::new(ErrorCode::Internal, error.to_string()))?;
    // Computed only for the local projection, below (C16): taking the pairing
    // mutex and building `PendingPairing` vecs on every remote poll was work the
    // remote projections then discarded.
    match conn_peer {
        Some(ConnPeer::Remote {
            scope: crate::peer_policy::PeerScope::PeerDevice,
            ..
        }) => {
            let peers = records
                .iter()
                .map(|record| devboule_protocol::PeerRow {
                    device_id: record.device_id.clone(),
                    display_name: record.display_name.clone(),
                    role: Some(devboule_protocol::projected_role(record.legacy_dialable)),
                    public_key: String::new(),
                    key_fingerprint: String::new(),
                    binding_kind: String::new(),
                    binding_node_name: None,
                    binding_login_name: None,
                    address: String::new(),
                    paired_at: record.paired_at,
                    revoked_at: record.revoked_at,
                    caps: Vec::new(),
                    paired_by_user: None,
                    online: state.is_peer_online(&record.device_id),
                })
                .collect();
            Ok(DevicesReply {
                self_info: devboule_protocol::SelfInfo {
                    device_id: identity.device_id.clone(),
                    display_name: identity.display_name.clone(),
                    public_key: String::new(),
                    key_fingerprint: String::new(),
                    addresses: Vec::new(),
                    port: 0,
                    daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                    protocol_version: PROTOCOL_VERSION,
                    // Withheld from a machine peer by **value**: the empty
                    // string, the empty array and `0` in the four fields above,
                    // and `remote` by omission (the one field `RemoteState`
                    // cannot express as "unknown"). The keys stay present
                    // because the 1b contract types them as required and the
                    // panel reads them unconditionally (C4).
                    remote: None,
                },
                peers,
                // A peer has no business deciding this device's pairings.
                pending: Vec::new(),
            })
        }
        Some(ConnPeer::Remote {
            scope: crate::peer_policy::PeerScope::PairedUser,
            ..
        }) => Ok(DevicesReply {
            self_info: devboule_protocol::SelfInfo {
                device_id: identity.device_id.clone(),
                display_name: identity.display_name.clone(),
                public_key: identity.public_key_b64(),
                key_fingerprint: identity.key_fingerprint.clone(),
                // Empty rather than absent, for the same reason as the Daemon
                // arm: where this device sits on the tailnet is not a client's
                // business, and the panel must still be able to read the field.
                addresses: Vec::new(),
                port: 0,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: PROTOCOL_VERSION,
                remote: None,
            },
            peers: records
                .iter()
                .map(|record| {
                    let mut row = crate::pairing::peer_row(state, record);
                    // The SID this device paired the peer from is local-only.
                    row.paired_by_user = None;
                    row
                })
                .collect(),
            pending: Vec::new(),
        }),
        _ => Ok(DevicesReply {
            self_info: devboule_protocol::SelfInfo {
                device_id: identity.device_id.clone(),
                display_name: identity.display_name.clone(),
                public_key: identity.public_key_b64(),
                key_fingerprint: identity.key_fingerprint.clone(),
                addresses: state.remote_addresses(),
                port: state.remote_port().unwrap_or(0),
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: PROTOCOL_VERSION,
                remote: Some(state.remote_state()),
            },
            peers: records
                .iter()
                .map(|record| crate::pairing::peer_row(state, record))
                .collect(),
            pending: state.pairing().pending_snapshot(),
        }),
    }
}

/// How long `pairing_address` waits for the accept thread to land a proposed
/// address set: the loop wakes within one housekeeping tick and the bind is
/// the cheap half of a swap, so this covers a first attempt and a first
/// failure's reason.
#[cfg(not(windows))]
const ADDRESS_SWAP_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// Where this device can be reached, for the code it displays.
fn pairing_address(state: &Arc<ServerState>) -> Result<String, WireError> {
    // A pairing code shows where to type in: probe the tailnet now and, when
    // its addresses differ from the bound set, propose the swap and wait
    // (bounded) for the accept thread to land it. A failed or slow bind
    // returns the reason; a stale address is never handed out.
    #[cfg(not(windows))]
    {
        let _ = state.ensure_remote_listener();
        if let Some(probed) = state.peer_transport().current_addresses() {
            if !same_addresses(&probed, &state.remote_addresses())
                && state.propose_addresses(probed.clone())
            {
                let deadline = std::time::Instant::now() + ADDRESS_SWAP_WAIT;
                loop {
                    if same_addresses(&probed, &state.remote_addresses()) {
                        break;
                    }
                    let remote = state.remote_state();
                    if remote.state == devboule_protocol::RemoteStateKind::Disabled {
                        return Err(WireError::new(
                            ErrorCode::Internal,
                            remote.reason.unwrap_or_else(|| {
                                "The tailnet address could not be bound.".to_string()
                            }),
                        ));
                    }
                    if std::time::Instant::now() >= deadline {
                        return Err(WireError::new(
                            ErrorCode::Internal,
                            "The new tailnet address did not bind within two seconds; show a code again.",
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        }
    }
    // No address yet? The listener is started at daemon start-up, but Tailscale
    // may have come up since (or been started precisely because the panel said
    // to). Try once more before refusing (C5), so the instruction this error
    // carries — start Tailscale and show a code again — is one the daemon
    // actually honours.
    if let Some(address) = remote_address(state) {
        return Ok(address);
    }
    if !state.ensure_remote_listener() {
        return Err(no_tailnet_address());
    }
    match remote_address(state) {
        Some(address) => Ok(address),
        None => Err(no_tailnet_address()),
    }
}

/// The `ip:port` this device advertises for pairing, when it has one. A
/// human types this verbatim into the far device's pairing field, so it is
/// composed by `SocketAddr` — hand-composing loses the brackets on IPv6 and
/// the text is no longer an address.
fn remote_address(state: &Arc<ServerState>) -> Option<String> {
    let addresses = state.remote_addresses();
    let ip: std::net::IpAddr = addresses.first()?.parse().ok()?;
    let port = state.remote_port()?;
    Some(crate::peer_transport::compose_peer_address(ip, port))
}

/// Whether the probed set is the bound one, as sets — the order of either
/// list carries no meaning.
#[cfg(not(windows))]
fn same_addresses(probed: &[std::net::IpAddr], bound: &[String]) -> bool {
    let mut probed: Vec<std::net::IpAddr> = probed.to_vec();
    let mut bound: Vec<std::net::IpAddr> = bound.iter().filter_map(|ip| ip.parse().ok()).collect();
    probed.sort_unstable();
    bound.sort_unstable();
    probed == bound
}

fn no_tailnet_address() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        // The remedy is real: showing a code again goes through
        // `pairing_address`, which retries the listener. Nothing here tells the
        // user to restart the daemon.
        "This device has no tailnet address to pair over. Start Tailscale, then show a code again.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The address a human must type on the other device is composed by
    /// `SocketAddr`, so an IPv6 tailnet address arrives bracketed and parses
    /// back. Hand-composing `ip:port` yields text no `SocketAddr` accepts, and
    /// the person at the far end types it verbatim into the pairing field.
    #[test]
    fn the_advertised_pairing_address_parses_on_ipv6() {
        let dir = crate::test_dirs::test_temp_dir(&format!(
            "devboule devices {:?}",
            std::thread::current().id()
        ));
        let server = ServerState::with_paths(
            "devices-test".into(),
            crate::paths::RuntimePaths::from_dir(&dir),
        )
        .expect("state");
        let ip: std::net::IpAddr = "fd7a:115c:a1e0::1".parse().expect("ip");
        server.set_remote_state(RemoteState::Enabled {
            addresses: vec![ip],
            port: 47831,
        });

        let address = remote_address(&server).expect("an advertised address");
        assert_eq!(
            address.parse::<std::net::SocketAddr>(),
            Ok(std::net::SocketAddr::new(ip, 47831)),
            "the advertised address must be the bracketed listener: {address}"
        );

        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The pairing panel's "now": `pairing_address` probes the tailnet,
    /// proposes a changed set and waits for the accept thread to bind it —
    /// so the code it returns is the new address; and when the bind never
    /// lands it returns the failure, never the stale address.
    #[cfg(not(windows))]
    #[test]
    fn pairing_address_waits_for_a_proposed_swap_and_refuses_a_stale_one() {
        use crate::peer_policy::TransportBinding;
        use crate::peer_transport::{
            BindingError, PeerListener, PeerTable, PeerTransport, RejectReason, Tailnet,
        };
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        struct ProposedSwap {
            fail_bind: bool,
        }
        impl PeerTransport for ProposedSwap {
            fn listen(
                &self,
                _paths: &crate::paths::RuntimePaths,
                stop: Arc<AtomicBool>,
            ) -> std::io::Result<PeerListener> {
                Tailnet::bind_peer_listener(&["127.0.0.1".parse().expect("loopback")], 0, stop)
            }
            fn current_addresses(&self) -> Option<Vec<std::net::IpAddr>> {
                // Not 127.0.0.2: macOS binds only 127.0.0.1 on lo0.
                Some(vec!["::1".parse().expect("loopback")])
            }
            fn bind_addresses(
                &self,
                addresses: &[std::net::IpAddr],
                stop: Arc<AtomicBool>,
            ) -> std::io::Result<PeerListener> {
                if self.fail_bind {
                    return Err(std::io::Error::other("synthetic bind failure"));
                }
                Tailnet::bind_peer_listener(addresses, 0, stop)
            }
            fn pre_noise_filter(
                &self,
                _peer: &std::net::SocketAddr,
                _peers: &PeerTable,
            ) -> Result<(), RejectReason> {
                Ok(())
            }
            fn binding(
                &self,
                _peer: &std::net::SocketAddr,
            ) -> Result<TransportBinding, BindingError> {
                Err(BindingError::Mismatch)
            }
        }

        let build = |tag: &str, fail_bind: bool| -> (std::path::PathBuf, Arc<ServerState>) {
            let dir = crate::test_dirs::test_temp_dir(tag);
            let server = ServerState::with_paths(
                format!("{tag}-state"),
                crate::paths::RuntimePaths::from_dir(&dir),
            )
            .expect("state");
            // The worker stays out of this test: pairing probes for itself.
            server.set_address_refresh_interval(std::time::Duration::from_secs(60));
            assert!(server
                .set_peer_transport(Arc::new(ProposedSwap { fail_bind }))
                .is_ok());
            (dir, server)
        };

        let (dir, server) = build("devboule-pairing-swap", false);
        let address = pairing_address(&server).expect("the swap lands");
        assert!(
            address.starts_with("[::1]:"),
            "the proposed set is the one advertised: {address}"
        );
        server.stop_remote_listener();
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);

        let (dir, server) = build("devboule-pairing-stale", true);
        let error = pairing_address(&server).expect_err("no address for a failed bind");
        assert!(
            error.message.contains("synthetic bind failure"),
            "{}",
            error.message
        );
        server.stop_remote_listener();
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
