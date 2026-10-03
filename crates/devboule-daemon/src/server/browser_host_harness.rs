//! A real daemon on a private named pipe, for the browser-host tests that
//! must cross a connection: the accept loop, `handle_client`, the framing and
//! the broker, with nothing stubbed between a host and the caller.

#![cfg(windows)]

use super::*;
use crate::client::DaemonClient;
use devboule_protocol::{Capability, ClientHello};

pub(crate) struct WireDaemon {
    paths: RuntimePaths,
    pub(crate) state: Arc<ServerState>,
    shutdown: transport::ListenerShutdown,
    accept: Option<JoinHandle<()>>,
}

impl WireDaemon {
    pub(crate) fn start(label: &str) -> Self {
        let dir = crate::test_dirs::test_temp_dir(label);
        let paths = RuntimePaths::from_dir(dir);
        paths.ensure_dir().expect("runtime dir");
        let state =
            ServerState::with_paths(format!("browser-wire-{label}"), paths.clone()).expect("state");
        let (listener, shutdown) =
            transport::bind(&paths, Arc::clone(&state.stop)).expect("bind listener");
        let accept_state = Arc::clone(&state);
        let accept = std::thread::Builder::new()
            .name("browser-wire-accept".into())
            .spawn(move || accept_loop(listener, accept_state))
            .ok();
        Self {
            paths,
            state,
            shutdown,
            accept,
        }
    }

    pub(crate) fn hello(browser_host: bool) -> ClientHello {
        let owner = OwnerId::new(
            crate::security::current_user_sid().expect("sid"),
            format!("browser-wire-{}", std::process::id()),
        )
        .expect("owner");
        let mut hello = ClientHello::m3a(owner, "devboule-test");
        if browser_host {
            hello.capabilities.push(Capability::new(caps::BROWSER_HOST));
        }
        hello
    }

    /// A connection spoken to frame by frame, past its hello.
    pub(crate) fn raw(&self, browser_host: bool) -> Framed {
        let file = transport::connect_pipe(&self.paths.pipe_name).expect("connect");
        let framed = Framed::new(file);
        framed
            .send(&ClientMessage::Hello(Self::hello(browser_host)))
            .expect("send hello");
        match framed
            .recv_timeout::<DaemonMessage>(Duration::from_secs(5))
            .expect("hello reply")
        {
            DaemonMessage::Hello(_) => framed,
            other => panic!("expected the daemon's hello, got {other:?}"),
        }
    }

    pub(crate) fn client(&self, browser_host: bool) -> DaemonClient {
        crate::client::connect(&self.paths, Self::hello(browser_host)).expect("connect")
    }
}

impl Drop for WireDaemon {
    fn drop(&mut self) {
        self.shutdown.shutdown();
        self.state.stop.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        let _ = std::fs::remove_dir_all(&self.paths.dir);
    }
}
