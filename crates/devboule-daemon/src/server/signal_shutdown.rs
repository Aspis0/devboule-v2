//! Unix termination signals → the shutdown the RPC path sends.
//!
//! SIGTERM and SIGINT would otherwise take the kernel's default route and
//! kill the process before the journal drain, the goodbye record and the
//! socket cleanup run — a deliberate stop that looks exactly like a crash.
//! The handler runs on whatever thread the kernel picked, so it writes only
//! what signal-hook's self-pipe allows and a dedicated thread turns the
//! arrival into the same `request_shutdown` the Shutdown RPC answers with.

use std::sync::Arc;
use std::thread::JoinHandle;

use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

use super::ServerState;

/// One run's signal wiring: while it lives, SIGTERM and SIGINT ask for the
/// graceful shutdown. Dropping it unregisters the handlers (the last
/// instance restores the default disposition) and joins the reader.
pub(super) struct SignalShutdown {
    handle: signal_hook::iterator::Handle,
    reader: JoinHandle<()>,
}

impl SignalShutdown {
    pub(super) fn install(state: Arc<ServerState>) -> Option<Self> {
        let mut signals = match Signals::new([SIGTERM, SIGINT]) {
            Ok(signals) => signals,
            Err(error) => {
                eprintln!("daemon could not watch for SIGTERM/SIGINT: {error}");
                return None;
            }
        };
        let handle = signals.handle();
        let reader = match std::thread::Builder::new()
            .name("daemon-signal".into())
            .spawn(move || {
                for _signal in signals.forever() {
                    state.request_shutdown();
                }
            }) {
            Ok(reader) => reader,
            Err(error) => {
                // `signals` drops on this path, so the half-installed hook
                // unregisters before the daemon continues without it.
                eprintln!("daemon could not start its signal thread: {error}");
                return None;
            }
        };
        Some(Self { handle, reader })
    }
}

impl Drop for SignalShutdown {
    fn drop(&mut self) {
        // Close first: the reader's `forever` ends on it, the join below
        // succeeds, and the `Signals` dropped inside that thread unregisters
        // the hook and restores the default disposition.
        self.handle.close();
        let _ = self.reader.join();
    }
}
