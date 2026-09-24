//! The idle-shutdown slot of an agent-created child: the wire create road's
//! `session_started`/`session_finished` pairing, on the road that has no client
//! frame (`SessionRegistry::create_session_for_agent`).
//!
//! Every test asserts `ServerState::live_session_count` directly. A slot is
//! always held by the creator first, so a missing increment and an extra
//! decrement both read as a wrong number instead of hiding behind zero's
//! saturation — and the creator's slot is the one a live session loses when the
//! daemon arms its idle exit on a count that reached zero.

use super::session_resume_fixture::{entry_present, AcpEnv};
use super::tests::insert_live_agent;
use super::*;
use devboule_protocol::compose_session_id;
use std::time::{Duration, Instant};

/// A daemon with one live creator session and its own lifecycle slot taken the
/// way the wire create road takes it. The creator is a registry entry, not a
/// process: what these tests measure is the child's road, and the creator only
/// has to be live and counted.
struct SlotFixture {
    dir: std::path::PathBuf,
    state: Arc<ServerState>,
    owner: OwnerId,
    creator: String,
    /// The prompt path gates on the broker's authenticated `tools/list`, and a
    /// unit test has no `run_windows` to start the listener the provider dials.
    _mcp_server: crate::mcp_broker::McpServerHandle,
}

impl SlotFixture {
    fn new(label: &str) -> Self {
        Self::build(label, None)
    }

    /// The same fixture, with standing instructions already in the runtime
    /// directory: the profiles store the state attaches reads its file once,
    /// at construction, so the document is on disk *before* the state exists.
    fn new_with_standing(label: &str, standing: &str) -> Self {
        Self::build(label, Some(standing))
    }

    fn build(label: &str, standing: Option<&str>) -> Self {
        let dir = crate::test_dirs::test_temp_dir("devboule-child-slot");
        // The user-provider rows are process-global (`user_providers::ROWS_STATE`),
        // and a refresh from a directory with no providers file retires whatever
        // another test just loaded. This fixture needs no rows of its own, so its
        // directory carries a document the store refuses — written *before* the
        // state exists, because constructing the state is itself a refresh. A
        // refusal keeps the loaded rows (the module's own rule, and the reason the
        // file is not absent), which makes every refresh this state triggers a
        // no-op for a test running beside it.
        std::fs::write(
            dir.join(crate::user_providers::PROVIDERS_FILE),
            b"{ this fixture declares no providers",
        )
        .expect("the fixture's providers document");
        if let Some(standing) = standing {
            // The same on-disk trick for the profiles store: the document the
            // state will load and attach carries the standing instructions a
            // first prompt is composed from.
            let document = devboule_protocol::AgentProfilesDocument {
                standing_instructions: standing.to_string(),
                ..devboule_protocol::AgentProfilesDocument::default()
            };
            std::fs::write(
                dir.join(crate::agent_profiles::PROFILES_FILE),
                serde_json::to_vec(&document).expect("the fixture's profiles document"),
            )
            .expect("write the profiles document");
        }
        let state = ServerState::with_paths(
            format!("child-slot-{label}"),
            RuntimePaths::from_dir(dir.clone()),
        )
        .expect("the fixture's daemon state");
        let owner = OwnerId::new("child-slot-user", format!("child-slot-{label}")).expect("owner");
        // The state's registry owns the same directory: no second path to keep in
        // step.
        let dir = state.sessions.runtime_dir().to_path_buf();
        let creator = compose_session_id(&owner.session_token(), "creator").expect("id");
        let journal = state.sessions.journal.as_ref().expect("journal").clone();
        journal
            .upsert_blocking(crate::journal::new_session_record(
                &creator,
                owner.user.clone(),
                None,
                SessionKind::Acp,
                "Slot creator",
            ))
            .expect("the creator's row");
        let _ = insert_live_agent(&state.sessions, &creator, owner.clone());
        assert!(
            state.session_started(),
            "the creator's own slot, taken the way the wire create road takes it"
        );
        let mcp_server = state.mcp.start(&state).expect("the MCP listener starts");
        Self {
            dir,
            state,
            owner,
            creator,
            _mcp_server: mcp_server,
        }
    }

    /// The child the broker's `devboule_create_agent` would ask for, with the
    /// preset facts the ACP override harness serves. An empty `prompt` is a
    /// child whose first prompt never goes out.
    fn create(&self, prompt: &str) -> Result<Session, WireError> {
        self.create_full(prompt, "", |_sessions| {})
    }

    /// The same creation with the resolved profile's spawn prompt in the
    /// `AgentCreation`: the road under test carries it from the resolution
    /// into the child's first prompt.
    fn create_with_spawn(&self, prompt: &str, spawn_prompt: &str) -> Result<Session, WireError> {
        self.create_full(prompt, spawn_prompt, |_sessions| {})
    }

    /// `arm` runs after the reservation and before the creation, which is the
    /// window a creator's own close lands in.
    fn create_armed(
        &self,
        prompt: &str,
        arm: impl FnOnce(&SessionRegistry),
    ) -> Result<Session, WireError> {
        self.create_full(prompt, "", arm)
    }

    fn create_full(
        &self,
        prompt: &str,
        spawn_prompt: &str,
        arm: impl FnOnce(&SessionRegistry),
    ) -> Result<Session, WireError> {
        let creator = self
            .state
            .sessions
            .agent_creator(&self.creator, &self.owner)
            .expect("the creator's own facts");
        let ticket = self
            .state
            .sessions
            .reserve_agent_creation(&self.creator, 1)
            .expect("a creation slot");
        arm(&self.state.sessions);
        self.state.sessions.create_session_for_agent(
            &self.state,
            AgentCreation {
                creator_session_id: self.creator.clone(),
                creator,
                creator_runtime: self.state.sessions.live_runtime(&self.creator, &self.owner),
                display_name: "slot child".to_string(),
                provider: "devboule-acp-stub".to_string(),
                profile_id: "profile-slot".to_string(),
                profile_name: "Slot child".to_string(),
                spawn_prompt: spawn_prompt.to_string(),
                delivery: crate::profile_delivery::ProfileDelivery::none(),
                overlay: crate::provider_catalog::ToolOverlay::NONE,
                labels: Default::default(),
                context_id: None,
                depth: 1,
                cwd: None,
                initial_prompt: prompt.to_string(),
                notify: false,
                workspace_id: None,
            },
            ticket,
        )
    }

    fn count(&self) -> u32 {
        self.state.live_session_count()
    }

    /// The child's entry leaving the map is the end the reader thread observed.
    fn wait_until_gone(&self, child: &str, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while entry_present(&self.state.sessions, child) {
            assert!(Instant::now() < deadline, "the entry never left: {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The decrement is the last act of the reader thread, after the removal, so
    /// the count is awaited rather than read through that window.
    fn wait_for_count(&self, expected: u32, what: &str) {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let count = self.count();
            if count == expected {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "the count is {count}, not {expected}: {what}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(self) {
        if let Some(journal) = self.state.sessions.journal.as_ref() {
            journal.shutdown();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The increment: a child an agent created holds a slot of its own beside the
/// creator's.
/// Mutant: `session_started()` dropped from `create_session_for_agent` — the
/// count reads 1 (the creator alone) with a live child on the map.
#[test]
fn an_agent_created_child_holds_a_lifecycle_slot() {
    let _env = AcpEnv::stub(&[]);
    let fixture = SlotFixture::new("live");
    let child = fixture.create("say hello").expect("the child is created");
    assert_eq!(
        fixture.count(),
        2,
        "the child's own slot is counted beside the creator's"
    );
    let _ = fixture
        .state
        .sessions
        .close(&child.id, &fixture.owner, &None);
    fixture.finish();
}

/// The killing sequence the arithmetic used to cancel: a counted session whose
/// client is gone, one agent child that starts and dies. The creator's slot
/// must survive the child's whole life — 2 while the child lives, 1 after its
/// provider exits — and the idle exit must not arm, because a count that
/// reaches 0 while the creator is still live is what arms the exit that
/// terminates it.
/// Mutant: the increment dropped — the count reads 1 where the child is live;
/// with that assertion disabled (measured), the child's EOF takes the creator's
/// slot with it ("the count is 0, not 1"), and with the count wait disabled as
/// well (measured) the idle exit arms below.
/// Mutant: the EOF decrement dropped (`session_spawn.rs`) — the count stays 2
/// and the post-death value never lands.
#[test]
fn the_creators_slot_survives_its_childs_birth_and_death() {
    // The child dies the instant its first prompt arrives, and the creation's
    // own prompt is empty on purpose: the child lives until this test sends the
    // prompt that kills it.
    let env = AcpEnv::stub(&[("DEVBOULE_ACP_STUB_EXIT_ON_PROMPT", "1".to_string())]);
    let fixture = SlotFixture::new("killed");
    let child = fixture.create("").expect("the child is created alive");
    // The child is spawned and carries its own environment: the override has
    // done its work, and the wait below must not hold the ACP tests' lock.
    drop(env);
    assert_eq!(fixture.count(), 2, "the live child holds its slot");

    // The prompt that ends it, on the ordinary send path: the provider exits on
    // reading it and the reader's EOF removes the entry.
    let conn = ConnHandle::with_peer(0, None);
    let _ = fixture
        .state
        .sessions
        .send_with_subscription_timeout(&SendRequest {
            session_id: &child.id,
            subscription_id: 0,
            text: "finish",
            attachments: &[],
            attachment_references: &[],
            owner: &fixture.owner,
            conn: &conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: false,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
        })
        .expect("the prompt reaches the living child");

    fixture.wait_until_gone(&child.id, "the child's provider exit");
    fixture.wait_for_count(1, "the child's slot came back, the creator's left");

    // The counter's failure mode is the arming, not the number: the grace is
    // `IDLE_SHUTDOWN_GRACE`, so a daemon still up after twice that did not arm.
    // A count read is not enough on its own — a wrong count also reads 1 until
    // the decrement lands (measured: with the increment dropped, the wait above
    // passes before it), and this is the assertion that watches the consequence.
    let deadline = Instant::now() + crate::IDLE_SHUTDOWN_GRACE * 2;
    while Instant::now() < deadline {
        assert!(
            !fixture.state.stop_flag().load(Ordering::SeqCst),
            "the idle exit armed while the creator's slot was still counted"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
    fixture.finish();
}

/// The creation hand-off, on the road itself: the spawn prompt carried in the
/// `AgentCreation` — what `resolve_profile` copied out of the store — is what
/// the creation send puts in front of the child, in the fixed order (standing
/// instructions, spawn prompt, creation preamble, creator's prompt), journaled
/// on the child as its `creation` line. A hand-off that passed `None` would
/// leave the spawn text out of the composed first prompt entirely.
#[test]
fn a_created_childs_first_prompt_carries_the_spawn_prompt_in_order() {
    let _env = AcpEnv::stub(&[]);
    let fixture = SlotFixture::new_with_standing("spawn-order", "standing");

    let child = fixture
        .create_with_spawn("the task", "spawn")
        .expect("the child is created and prompted");
    let journal = fixture
        .state
        .sessions
        .journal
        .as_ref()
        .expect("journal")
        .clone();
    let expected = format!(
        "standing

spawn

{}

the task",
        crate::provider_catalog::AGENT_PREAMBLE
    );
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut found: Option<String> = None;
    while found.is_none() && Instant::now() < deadline {
        if let Ok(replay) = journal.replay(&child.id) {
            for event in &replay.events {
                if let SessionEvent::AgentUserMessage {
                    text,
                    message_kind: UserMessageKind::Creation,
                    ..
                } = event
                {
                    found = Some(text.clone());
                }
            }
        }
        if found.is_none() {
            std::thread::sleep(Duration::from_millis(25));
        }
    }
    assert_eq!(
        found.as_deref(),
        Some(expected.as_str()),
        "standing, then the profile's spawn prompt, then the preamble, then the task"
    );
    fixture.finish();
}

/// A creation that failed inside the spawn never left a live entry, so the slot
/// the gate took goes back.
/// Mutant: the `Err` arm's `session_finished()` dropped — the count reads 2
/// with no child on the map, and the daemon never arms idle shutdown again.
#[test]
fn a_failed_spawn_gives_back_the_slot_the_gate_took() {
    let _env = AcpEnv::missing_agent();
    let fixture = SlotFixture::new("failed-create");
    let error = fixture.create("say hello").expect_err("the spawn fails");
    assert!(
        error.message.starts_with("Could not start ACP agent"),
        "{error:?}"
    );
    assert_eq!(fixture.count(), 1, "the creator's slot is all that is left");
    fixture.finish();
}

/// The creator closes while its child is starting: the child is closed, and the
/// slot its creation took comes back with it.
/// Mutant: the abandon arm's paired `session_finished()` dropped — the count
/// reads 2 with the child closed, and the daemon never arms idle shutdown again.
#[test]
fn an_abandoned_child_gives_back_its_slot() {
    let _env = AcpEnv::stub(&[]);
    let fixture = SlotFixture::new("abandoned");
    // The commit's own refusal: the creator's caps row is marked gone between
    // the reservation and the creation, which is the state the creator closing
    // mid-start leaves behind.
    let error = fixture
        .create_armed("say hello", |sessions| {
            sessions.forget_agent_creator(&fixture.creator)
        })
        .expect_err("the creator is gone");
    assert_eq!(error.message, "creator closed", "the abandon arm answered");
    assert_eq!(fixture.count(), 1, "the abandoned child's slot came back");
    fixture.finish();
}

/// The prompt-failure arm: a child whose first prompt is refused while it is
/// still live is closed, and the close releases the slot the gate took.
/// Mutant: the arm's paired `session_finished()` dropped — the count reads 2
/// with the child's entry gone, and the daemon never arms idle shutdown again.
#[test]
fn a_refused_first_prompt_gives_back_its_slot() {
    // The provider delays its MCP probe past the daemon's readiness timeout, so
    // the send is refused with the provider still alive and its entry on the
    // map — the state the close below is for.
    let _env = AcpEnv::stub(&[
        ("DEVBOULE_ACP_STUB_MCP_DELAY_MS", "5000".to_string()),
        ("DEVBOULE_MCP_READY_TIMEOUT_MS", "100".to_string()),
    ]);
    let fixture = SlotFixture::new("refused-prompt");
    let error = fixture
        .create("say hello")
        .expect_err("the prompt is refused");
    assert!(
        error
            .message
            .contains("did not receive an authenticated tools/list"),
        "the send refused the prompt, not the spawn: {error:?}"
    );
    assert_eq!(fixture.count(), 1, "the closed child's slot came back");
    fixture.finish();
}

/// A creator's `devboule_close_agent` removes a live child through the registry
/// verb, not through the wire handler that releases the slot for a client.
/// Mutant: the release dropped from `close_agent_child` — the count reads 2 with
/// the child's row gone, and the daemon never arms idle shutdown again.
#[test]
fn a_creator_that_closes_its_child_releases_its_slot() {
    let _env = AcpEnv::stub(&[]);
    let fixture = SlotFixture::new("closed-child");
    let child = fixture.create("say hello").expect("the child is created");
    assert_eq!(fixture.count(), 2, "the live child holds its slot");
    let removed = fixture
        .state
        .sessions
        .close_agent_child(&fixture.state, &fixture.creator, &child.id)
        .expect("the creator closes its own child");
    assert!(removed, "a live child was removed");
    assert_eq!(fixture.count(), 1, "the removed child's slot came back");
    fixture.finish();
}
