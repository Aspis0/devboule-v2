//! Shared spawn/restart harness for the Claude resume battery.
//!
//! A trimmed copy of the ACP suite's harness: a real daemon binary on a temp
//! runtime dir, killed hard on restart (the daemon-death case), with the
//! `DEVBOULE_CLAUDE_COMMAND` override carried in the process environment so
//! the respawned daemon inherits it.
#![allow(dead_code)]

use std::path::PathBuf;
use std::process::Child;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect, current_user_sid, spawn_daemon, DaemonClient, EventHandler, RuntimePaths,
};
use devboule_protocol::{ClientHello, OwnerId, SessionEvent};

static TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn file_secret_store() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("DEVBOULE_SECRET_STORE", "file"));
}

pub fn daemon_bin() -> PathBuf {
    file_secret_store();
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    panic!("CARGO_BIN_EXE_devboule-daemon was not provided by Cargo");
}

pub fn claude_stub_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-claude-stub") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-claude-stub") {
        return PathBuf::from(path);
    }
    panic!("CARGO_BIN_EXE_devboule-claude-stub was not provided by Cargo");
}

fn unique_dir(prefix: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "{prefix} {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("runtime directory");
    dir
}

fn hello(name: &str) -> ClientHello {
    let sid = current_user_sid().expect("current user SID");
    ClientHello::m3a(
        OwnerId::new(sid, format!("claude-resume-{name}-{}", std::process::id())).expect("owner"),
        "devboule-claude-resume-test",
    )
}

pub struct EnvGuard {
    names: Vec<&'static str>,
    restore: Vec<(&'static str, std::ffi::OsString)>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for name in &self.names {
            std::env::remove_var(name);
        }
        for (name, value) in &self.restore {
            std::env::set_var(name, value);
        }
    }
}

pub struct Harness {
    pub dir: PathBuf,
    pub paths: RuntimePaths,
    child: Option<Child>,
}

impl Harness {
    pub fn spawn() -> Self {
        let dir = unique_dir("devboule claude resume");
        let paths = RuntimePaths::from_dir(&dir);
        let child = spawn_daemon(&daemon_bin(), &paths).expect("spawn daemon");
        let harness = Self {
            dir,
            paths,
            child: Some(child),
        };
        harness.wait_until_up();
        harness
    }

    fn wait_until_up(&self) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if connect(&self.paths, hello("wait")).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("daemon did not start");
    }

    pub fn client(&self) -> DaemonClient {
        self.client_named("client")
    }

    pub fn client_named(&self, name: &str) -> DaemonClient {
        connect(&self.paths, hello(name)).expect("connect")
    }

    /// The daemon-death case: SIGKILL-equivalent, children with it (the Job
    /// Object), journal rows left `Live` for the next boot to recover.
    pub fn restart(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        std::thread::sleep(Duration::from_millis(150));
        self.child = Some(spawn_daemon(&daemon_bin(), &self.paths).expect("spawn daemon"));
        self.wait_until_up();
    }

    pub fn journal_path(&self) -> PathBuf {
        self.paths.journal_file()
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Point the daemon at the stub CLI. Process-wide (the respawn inherits it),
/// so callers hold the file lock. `home` becomes `USERPROFILE` for the run:
/// the stub files its fake conversation there, and the daemon's pre-resume
/// lookup reads the same place — the real `~/.claude` stays untouched.
pub fn use_stub_cli(
    argv_file: &std::path::Path,
    console_file: &std::path::Path,
    home: &std::path::Path,
) -> EnvGuard {
    let argv = vec![
        claude_stub_bin().to_string_lossy().into_owned(),
        "--argv-file".to_string(),
        argv_file.to_string_lossy().into_owned(),
        "--console-file".to_string(),
        console_file.to_string_lossy().into_owned(),
    ];
    std::env::set_var(
        "DEVBOULE_CLAUDE_COMMAND",
        serde_json::to_string(&argv).expect("stub argv"),
    );
    let restore = std::env::var_os("USERPROFILE")
        .map(|value| ("USERPROFILE", value))
        .into_iter()
        .collect();
    std::env::set_var("USERPROFILE", home);
    EnvGuard {
        names: vec!["DEVBOULE_CLAUDE_COMMAND"],
        restore,
    }
}

pub fn collect_events() -> (Arc<Mutex<Vec<SessionEvent>>>, EventHandler) {
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    (events, handler)
}

pub fn wait_for(
    events: &Mutex<Vec<SessionEvent>>,
    timeout: Duration,
    what: &str,
    mut done: impl FnMut(&[SessionEvent]) -> bool,
) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done(&events.lock().expect("events lock")) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for Claude event ({what})");
}
