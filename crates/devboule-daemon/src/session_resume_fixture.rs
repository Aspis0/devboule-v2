//! Test support for the resume road's three characterisation files: the
//! registry fixture they all build on, the ACP override harness the spawn arms
//! drive, and the two pollers the detached writes need. It holds no test of
//! its own — the claims live beside the tests that make them.

use super::*;

pub(super) struct ResumeFixture {
    pub(super) dir: std::path::PathBuf,
    pub(super) state: Arc<ServerState>,
    pub(super) owner: OwnerId,
}

impl ResumeFixture {
    pub(super) fn new(label: &str) -> Self {
        let state = ServerState::new(format!("resume-{label}"));
        let owner = OwnerId::new("s-1-5-21-resume", format!("resume-{label}")).expect("owner");
        let dir = state.sessions.runtime_dir().to_path_buf();
        Self { dir, state, owner }
    }

    pub(super) fn registry(&self) -> &SessionRegistry {
        &self.state.sessions
    }

    pub(super) fn journal(&self) -> &Arc<Journal> {
        self.state.sessions.journal.as_ref().expect("journal")
    }

    pub(super) fn id(&self, unique: &str) -> String {
        compose_session_id(&self.owner.session_token(), unique).expect("id")
    }

    pub(super) fn write_row(&self, row: SessionRecord) {
        self.journal().upsert_blocking(row).expect("row");
    }

    pub(super) fn row(&self, id: &str) -> SessionRecord {
        self.journal()
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == id)
            .expect("the row")
    }

    pub(super) fn resume(&self, id: &str, conn: &ConnHandle) -> Result<Session, WireError> {
        self.state
            .sessions
            .resume(&self.state, id, &self.owner, conn)
    }

    pub(super) fn conn(&self) -> Arc<ConnHandle> {
        ConnHandle::new(7)
    }

    pub(super) fn finish(self) {
        self.journal().shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An ACP row the resume gate admits: resumable kind, a named provider, and
/// the handle the far side was known by.
pub(super) fn acp_row(id: &str, owner: &OwnerId, handle: &str) -> SessionRecord {
    let mut row = new_session_record(
        id,
        owner.user.clone(),
        None,
        SessionKind::Acp,
        "Resumed agent",
    );
    row.provider = Some("devboule-acp-stub".to_string());
    row.peer_session_id = Some(handle.to_string());
    row
}

/// The row as the resume's lineage read sees an unreadable overlay cell: the
/// journal writer stores the deny names it is given, and the reader refuses a
/// name no broker tool serves (`deserialize_overlay`), so this row's overlay
/// comes back `None` — the same value a rotted cell produces — with a
/// `created_by` that makes the lineage read judge it.
pub(super) fn row_with_unreadable_overlay(
    id: &str,
    owner: &OwnerId,
    creator: &str,
    handle: &str,
) -> SessionRecord {
    let mut row = acp_row(id, owner, handle);
    row.created_by = Some(creator.to_string());
    row.overlay = Some(crate::provider_catalog::ToolOverlay::from_profile_names(&[
        "devboule_no_such_tool".to_string(),
    ]));
    row
}

/// One slot held by an unrelated live session before the call under test. With
/// it, a missing decrement and a stray increment both read as a wrong count
/// instead of hiding behind zero's saturation.
pub(super) fn take_bystander_slot(state: &Arc<ServerState>) {
    assert!(state.session_started(), "the daemon admits a session here");
}

pub(super) fn entry_present(registry: &SessionRegistry, id: &str) -> bool {
    registry.inner.lock().expect("registry").contains_key(id)
}

/// The ACP override the four pre-existing writers already serialise on: it is
/// process-global, so it is held for the whole test, and cleared on drop.
pub(super) struct AcpEnv {
    _guard: std::sync::MutexGuard<'static, ()>,
    names: Vec<&'static str>,
}

impl AcpEnv {
    /// A command that cannot start: every road these tests take must fail at
    /// the spawn, and resolving the command itself must still succeed.
    pub(super) fn missing_agent() -> Self {
        Self::set("devboule-no-such-agent", &[])
    }

    /// The real stub the crate's integration battery drives, with its knobs.
    pub(super) fn stub(extra: &[(&'static str, String)]) -> Self {
        let program = acp_stub();
        Self::set(
            &program.to_string_lossy(),
            &[("DEVBOULE_TEST_NO_NETWORK", "1".to_string())],
        )
        .with(extra)
    }

    fn set(program: &str, extra: &[(&'static str, String)]) -> Self {
        let guard = lock_acp_env();
        std::env::set_var(
            "DEVBOULE_ACP_COMMAND",
            serde_json::json!([program]).to_string(),
        );
        std::env::set_var("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub");
        let env = Self {
            _guard: guard,
            names: vec!["DEVBOULE_ACP_COMMAND", "DEVBOULE_ACP_PROVIDER_ID"],
        };
        env.with(extra)
    }

    fn with(mut self, extra: &[(&'static str, String)]) -> Self {
        for (name, value) in extra {
            std::env::set_var(name, value);
            self.names.push(name);
        }
        self
    }
}

impl Drop for AcpEnv {
    fn drop(&mut self) {
        for name in self.names.drain(..) {
            std::env::remove_var(name);
        }
    }
}

/// The stub sits beside the test binary because Cargo built both in this
/// invocation; refusing to guess is the point (a stale binary would test the
/// past, and skipping would turn the test into a false green).
fn acp_stub() -> std::path::PathBuf {
    let candidate = std::env::current_exe()
        .expect("test binary path")
        .parent()
        .and_then(std::path::Path::parent)
        .expect("target dir")
        .join(format!("devboule-acp-stub{}", std::env::consts::EXE_SUFFIX));
    assert!(
        candidate.is_file(),
        "the ACP stub is missing at {}; build it with `cargo build -p devboule-daemon \
         --bin devboule-acp-stub --features test-support`",
        candidate.display()
    );
    candidate
}

/// A `Transcript` entry for `id`, owned by `owner`: the registry's view of a
/// row that holds no process.
pub(super) fn insert_transcript(registry: &SessionRegistry, id: &str, owner: OwnerId) {
    let metadata = Session {
        id: id.to_string(),
        workspace_id: None,
        cwd: None,
        kind: SessionKind::Acp,
        title: "Recovered agent".to_string(),
        state: SessionState::Ended {
            generation: 1,
            code: Some(0),
            integrity: TranscriptIntegrity::Complete,
        },
        elapsed_ms: Some(0),
        provider: Some("devboule-acp-stub".to_string()),
        peer_session_id: Some("handle".to_string()),
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::No,
        labels: Default::default(),
        resumable: true,
    };
    let runtime = SessionRuntime::from_replay(
        id.to_string(),
        registry.journal.clone(),
        crate::journal::Replay {
            generation: 1,
            last_seq: 0,
            integrity: TranscriptIntegrity::Complete,
            event_seqs: Vec::new(),
            events: Vec::new(),
        },
    );
    registry.inner.lock().expect("registry").insert(
        id.to_string(),
        RegistryEntry::Transcript(Box::new(TranscriptSession {
            metadata,
            owner,
            runtime,
        })),
    );
}

/// The row as the writer's own view of it, waiting for a predicate the
/// detached thread's write satisfies.
pub(super) fn until_row(
    fixture: &ResumeFixture,
    id: &str,
    what: &str,
    done: impl Fn(&SessionRecord) -> bool,
) -> SessionRecord {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let row = fixture.row(id);
        if done(&row) {
            return row;
        }
        assert!(
            Instant::now() < deadline,
            "the journal never did: {what} {row:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn wait_for_child_finished(fixture: &ResumeFixture, creator: &str, child: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let _ = fixture.journal().flush();
        let events = fixture.journal().replay(creator).expect("replay").events;
        if events.iter().any(|event| {
            matches!(event, SessionEvent::ChildFinished { child_session_id, .. } if child_session_id == child)
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the child's end never reached the creator's journal"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}
