//! The restricted-child tests, moved whole out of `session_tests.rs` lines
//! 8255-8894 (at `f104df1`), the two fixtures at their head included: a
//! restricted child keeps both denials across a restart and after its profile
//! changes, an orphaned resume keeps the birth restriction, a row without a
//! recorded depth resumes unable to delegate, an unreadable overlay cell
//! refuses the resume but not the roster, a birth write carries the overlay and
//! the depth even when the spawn fails, a later upsert without birth facts
//! keeps them — and on the close/stop side, the child predicate, an agent
//! closing only its own children, a stop that refuses what a close refuses, and
//! a stop that ends the whole job tree. Every line below is byte-identical to
//! its text there apart from this header; `sorted_denied` and
//! `restricted_birth_record` sit above the tests and travel with them, so
//! nothing was promoted for this move.

use super::tests::{insert_child, insert_live_agent, test_owner, tmp_delete_registry};
use super::*;

/// The deny list as a sorted set: overlay semantics are order-free and the
/// write canonicalises, so tests compare sets, never byte order.
fn sorted_denied(overlay: &crate::provider_catalog::ToolOverlay) -> Vec<String> {
    let mut names = overlay.disabled_names();
    names.sort();
    names
}

/// The birth overlay of a restricted child, as the birth write writes it:
/// the deny pair resolved from the profile, on a row with a creator.
fn restricted_birth_record(
    id: &str,
    owner: &str,
    overlay: crate::provider_catalog::ToolOverlay,
) -> crate::journal::SessionRecord {
    let mut record = new_session_record(
        id.to_string(),
        owner.to_string(),
        None,
        SessionKind::Acp,
        "Agent",
    );
    record.created_by = Some("overlay-creator".to_string());
    record.profile_id = Some("profile-design".to_string());
    record.overlay = Some(overlay);
    // A child of a human root: depth 1, the value the birth write stamps.
    record.depth = Some(1);
    record
}

/// A restricted child keeps both denials across a daemon restart: the birth
/// row survives the reopen, and the resume mapping restores birth powers —
/// at depth, under the birth overlay — instead of the root's.
///
/// Surrogate, stated honestly: the restart is a journal reopen on the same
/// file (the layer that actually failed — the row had no overlay), not a
/// live provider respawn. What this does not prove is the respawned
/// provider re-running its MCP roundtrip in the child; the gates the lineage
/// feeds are pinned separately, on the exact functions the broker calls.
#[test]
fn a_restricted_child_keeps_its_birth_overlay_across_a_restart() {
    let (_dir, _registry, journal_a) = tmp_delete_registry();
    let owner = test_owner("overlay-restart-user", "overlay-restart-client");
    let birth_overlay = crate::provider_catalog::ToolOverlay::from_profile_names(&[
        crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
        crate::provider_catalog::MCP_CREATE_AGENT_TOOL.to_string(),
    ]);
    journal_a
        .create_session(restricted_birth_record(
            "overlay-child",
            &owner.user,
            birth_overlay.clone(),
        ))
        .expect("birth row");
    journal_a.shutdown();
    // The daemon after the restart: a new registry on the same file reads
    // the row the old one wrote.
    let journal_b = Arc::new(Journal::open(&_dir.join("journal.db")).expect("reopen"));
    let registry_b =
        SessionRegistry::new(RuntimePaths::from_dir(&_dir), Some(Arc::clone(&journal_b)));
    let row = registry_b
        .journal_roster()
        .expect("roster")
        .into_iter()
        .find(|row| row.id == "overlay-child")
        .expect("the birth row survived the restart");
    assert_eq!(
        sorted_denied(row.overlay.as_ref().expect("birth overlay")),
        [
            crate::provider_catalog::MCP_CREATE_AGENT_TOOL.to_string(),
            crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
        ],
        "the row kept the birth deny pair"
    );
    assert_eq!(row.depth, Some(1), "the row kept the birth depth");
    let lineage = SessionRegistry::resumed_lineage(Some(&row)).expect("readable row restores");
    assert_eq!(lineage.depth, 1);
    assert_eq!(
        lineage.overlay,
        row.overlay.clone().expect("birth overlay"),
        "resume carries the row's overlay verbatim"
    );
    assert!(
        !lineage
            .overlay
            .allows(crate::provider_catalog::MCP_SEND_MESSAGE_TOOL)
            && !lineage
                .overlay
                .allows(crate::provider_catalog::MCP_CREATE_AGENT_TOOL),
        "the restored lineage still denies both tools"
    );
    // No creator is consulted: with no row at all the lineage is root, and
    // that is the only root case left.
    assert_eq!(
        SessionRegistry::resumed_lineage(None).expect("no row is root"),
        crate::mcp_broker::AgentLineage::root(),
    );
    journal_b.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// The property the column is paid for: the profile is edited and then
/// deleted after the birth, and the resumed child still carries the overlay
/// it was born with. The resume read takes no store, so no edit can move
/// it — if a later pass re-resolves at resume, this test fails.
#[test]
fn a_resumed_child_keeps_its_birth_overlay_after_the_profile_changes() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("overlay-drift-user", "overlay-drift-client");
    let store = crate::agent_profiles::AgentProfilesStore::load(&_dir);
    // Birth: the profile denies send_message only, resolved the way birth
    // resolves it — from the store's deny list, once.
    store
        .set(devboule_protocol::AgentProfilesDocument {
            profiles: vec![devboule_protocol::AgentProfile {
                id: "p-birth".to_string(),
                name: "Birth".to_string(),
                icon: None,
                note: String::new(),
                spawn_prompt: String::new(),
                provider: "claude".to_string(),
                model: "claude-opus-4-6".to_string(),
                mode_id: "default".to_string(),
                thinking_option_id: None,
                features: serde_json::Map::new(),
                tool_overlay: vec![crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string()],
                enabled_for_agents: true,
                idle_close_minutes: None,
            }],
            standing_instructions: String::new(),
        })
        .expect("store the birth profile");
    let stored = store
        .document()
        .profiles
        .into_iter()
        .find(|profile| profile.name == "Birth")
        .expect("birth profile");
    let birth_overlay =
        crate::provider_catalog::ToolOverlay::from_profile_names(&stored.tool_overlay);
    let mut record =
        restricted_birth_record("overlay-drift-child", &owner.user, birth_overlay.clone());
    record.profile_id = Some("p-birth".to_string());
    journal.create_session(record).expect("birth row");
    // The human edits the profile (overlay cleared) and then deletes it.
    let mut edited = store.document();
    edited.profiles[0].tool_overlay.clear();
    store.set(edited).expect("clear the overlay");
    store
        .set(devboule_protocol::AgentProfilesDocument {
            profiles: Vec::new(),
            standing_instructions: String::new(),
        })
        .expect("delete the profile");
    assert!(
        store.document().profiles.is_empty(),
        "the profile is really gone: a re-resolution could not find it"
    );
    let row = registry
        .journal_roster()
        .expect("roster")
        .into_iter()
        .find(|row| row.id == "overlay-drift-child")
        .expect("birth row");
    let lineage = SessionRegistry::resumed_lineage(Some(&row)).expect("readable row restores");
    assert_eq!(
        lineage.overlay, birth_overlay,
        "birth wins over the edited, deleted store"
    );
    assert!(
        !lineage
            .overlay
            .allows(crate::provider_catalog::MCP_SEND_MESSAGE_TOOL),
        "send stays denied"
    );
    assert!(
        lineage
            .overlay
            .allows(crate::provider_catalog::MCP_CREATE_AGENT_TOOL),
        "create was never denied"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// The contrary the old test encoded as intended behavior: a resumed child
/// whose creator is gone keeps the restriction it was born with. Depth and
/// overlay are both birth facts — gating either on a live parent would let
/// an orphan resumed shallow delegate again, which is the escalation this
/// column exists to stop. Liveness decides only the bookkeeping.
#[test]
fn an_orphaned_resume_keeps_the_birth_restriction() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("overlay-orphan-user", "overlay-orphan-client");
    let birth_overlay = crate::provider_catalog::ToolOverlay::from_profile_names(&[
        crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
        crate::provider_catalog::MCP_CREATE_AGENT_TOOL.to_string(),
    ]);
    let mut record = restricted_birth_record("overlay-orphan", &owner.user, birth_overlay.clone());
    // Depth 2: born a grandchild. No creator is ever inserted live, so the
    // bookkeeping has nothing to count — the lineage must still come back
    // whole, or the cap launders through the orphan.
    record.depth = Some(2);
    journal.create_session(record).expect("birth row");
    let row = registry
        .journal_roster()
        .expect("roster")
        .into_iter()
        .find(|row| row.id == "overlay-orphan")
        .expect("birth row");
    let lineage = SessionRegistry::resumed_lineage(Some(&row)).expect("readable row restores");
    assert_eq!(lineage.depth, 2, "an orphan keeps its birth depth");
    assert_eq!(
        sorted_denied(&lineage.overlay),
        [
            crate::provider_catalog::MCP_CREATE_AGENT_TOOL.to_string(),
            crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
        ],
        "an orphan keeps its birth restriction"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// A row that predates the depth column resumes at the closed end of the
/// cap: it can work, but it cannot prove it is shallow enough to delegate.
/// Defaulting it to 1 would grant a depth nobody recorded.
#[test]
fn a_row_without_a_recorded_depth_resumes_unable_to_delegate() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("overlay-nodepth-user", "overlay-nodepth-client");
    let mut record = restricted_birth_record(
        "overlay-nodepth",
        &owner.user,
        crate::provider_catalog::ToolOverlay::NONE,
    );
    record.depth = None;
    journal.create_session(record).expect("birth row");
    let row = registry
        .journal_roster()
        .expect("roster")
        .into_iter()
        .find(|row| row.id == "overlay-nodepth")
        .expect("birth row");
    assert_eq!(row.depth, None);
    let lineage = SessionRegistry::resumed_lineage(Some(&row)).expect("readable row restores");
    assert_eq!(
        lineage.depth, MAX_AGENT_DEPTH,
        "unknown depth fails closed at the cap"
    );
    assert_eq!(lineage.overlay, crate::provider_catalog::ToolOverlay::NONE);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// An unreadable overlay cell refuses the resume instead of reading as
/// unrestricted — while the roster, which never reads the cell, keeps
/// listing the row. The message names the column.
#[test]
fn an_unreadable_overlay_cell_refuses_resume_but_not_the_roster() {
    let (_dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("overlay-rot-user", "overlay-rot-client");
    journal
        .create_session(restricted_birth_record(
            "overlay-rot",
            &owner.user,
            crate::provider_catalog::ToolOverlay::from_profile_names(&[
                crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
            ]),
        ))
        .expect("birth row");
    // Bit rot, by hand: the cell is no longer a deny list.
    rusqlite::Connection::open(_dir.join("journal.db"))
        .expect("open journal file")
        .execute(
            r#"UPDATE sessions SET overlay = '{"broken":' WHERE id = 'overlay-rot'"#,
            [],
        )
        .expect("rot the cell");
    let row = registry
        .journal_roster()
        .expect("the roster survives one bad cell")
        .into_iter()
        .find(|row| row.id == "overlay-rot")
        .expect("the row still lists");
    assert_eq!(row.overlay, None, "the damage travels, it does not default");
    let error = SessionRegistry::resumed_lineage(Some(&row)).expect_err("resume refuses");
    assert!(
        error.message.contains("overlay"),
        "the refusal names the column: {}",
        error.message
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

/// The write side through the real birth function: a creation whose spawn
/// fails after the birth door still leaves the overlay and depth the birth
/// resolved on the row. Reverting the two birth lines leaves NULLs here.
/// The spawn never runs — the binary does not exist — so the failure is a
/// fast OS error, never a hung handshake.
#[test]
fn a_birth_write_carries_overlay_and_depth_even_when_spawn_fails() {
    let state = ServerState::new("overlay-birth-write".to_string());
    let owner = test_owner("overlay-birth-user", "overlay-birth-client");
    let _acp_env = crate::session::lock_acp_env();
    std::env::set_var(
        "DEVBOULE_ACP_COMMAND",
        r#"["definitely-not-a-real-program-xyz"]"#,
    );
    let meta = SessionCreateMeta {
        overlay: crate::provider_catalog::ToolOverlay::from_profile_names(&[
            crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
            crate::provider_catalog::MCP_CREATE_AGENT_TOOL.to_string(),
        ]),
        depth: 2,
        display_name: Some("f1-birth-marker".to_string()),
        ..SessionCreateMeta::default()
    };
    let result = state.sessions.create_with_provider_env(
        &state,
        &owner,
        None,
        SessionKind::Acp,
        None,
        crate::profile_delivery::ProfileDelivery::none(),
        None,
        &None,
        None,
        &meta,
    );
    std::env::remove_var("DEVBOULE_ACP_COMMAND");
    assert!(result.is_err(), "the spawn must fail on the fake binary");
    let row = state
        .sessions
        .journal
        .as_ref()
        .expect("journal")
        .list()
        .expect("rows")
        .into_iter()
        .find(|row| row.display_name.as_deref() == Some("f1-birth-marker"))
        .expect("the birth row survived the failed spawn");
    assert_eq!(
        sorted_denied(row.overlay.as_ref().expect("birth overlay")),
        [
            crate::provider_catalog::MCP_CREATE_AGENT_TOOL.to_string(),
            crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
        ],
        "the birth write stamped the deny pair"
    );
    assert_eq!(row.depth, Some(2), "the birth write stamped the depth");
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    drop(state);
    let _ = std::fs::remove_dir_all(runtime_dir);
}

/// The two upsert lines: a later write without birth facts keeps the birth
/// values instead of clearing them. The clause has no production caller
/// today — births INSERT, everything else UPDATEs around it — but the
/// lines are the backstop if one ever does, so they are pinned, not trusted.
#[test]
fn a_later_upsert_without_birth_facts_keeps_them() {
    let (_dir, _registry, journal) = tmp_delete_registry();
    let owner = test_owner("overlay-upsert-user", "overlay-upsert-client");
    let mut birth = restricted_birth_record(
        "overlay-upsert",
        &owner.user,
        crate::provider_catalog::ToolOverlay::from_profile_names(&[
            crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string(),
        ]),
    );
    birth.depth = Some(2);
    journal.create_session(birth).expect("birth row");
    // An end-marker-style upsert carries no birth facts: both default to
    // nothing stated, which must read as "keep", never as "clear".
    let marker = new_session_record(
        "overlay-upsert".to_string(),
        owner.user.clone(),
        None,
        SessionKind::Acp,
        "Agent",
    );
    assert_eq!(marker.overlay, None);
    assert_eq!(marker.depth, None);
    journal.upsert_blocking(marker).expect("upsert");
    let row = journal
        .list()
        .expect("list")
        .into_iter()
        .find(|row| row.id == "overlay-upsert")
        .expect("row");
    assert_eq!(
        sorted_denied(row.overlay.as_ref().expect("birth overlay")),
        [crate::provider_catalog::MCP_SEND_MESSAGE_TOOL.to_string()],
        "the upsert kept the birth overlay"
    );
    assert_eq!(row.depth, Some(2), "the upsert kept the birth depth");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&_dir);
}

// ------------------------------------------------------------------
// An agent ends its own children: the shared scope gate behind
// `stop_agent_child` and `close_agent_child`, and the one spelling
// of the child predicate all of its callers read.
// ------------------------------------------------------------------

/// The predicate answers `created_by` alone: a child is a session whose
/// `created_by` names the caller. Same owner, same display name, nothing
/// else makes a child.
#[test]
fn the_child_predicate_answers_created_by_alone() {
    assert!(is_child_of(Some("creator"), "creator"));
    assert!(!is_child_of(None, "creator"));
    assert!(!is_child_of(Some("sibling"), "creator"));
}

#[test]
fn an_agent_closes_only_its_own_children() {
    // A real state, not a bare registry: `close_agent_child` releases the
    // child's idle-shutdown slot on the state the registry belongs to.
    let state = ServerState::new("end-children".to_string());
    let dir = state.sessions.runtime_dir().to_path_buf();
    let registry = state.sessions.clone();
    let journal = state.sessions.journal.clone().expect("journal");
    let owner = test_owner("end-children-user", "end-children-client");
    let stranger = test_owner("end-children-stranger", "end-children-stranger-client");
    let parent = compose_session_id(&owner.session_token(), "end-par").expect("id");
    let caller = compose_session_id(&owner.session_token(), "end-cal").expect("id");
    let child = compose_session_id(&owner.session_token(), "end-chi").expect("id");
    let human = compose_session_id(&owner.session_token(), "end-hum").expect("id");
    let foreign = compose_session_id(&stranger.session_token(), "end-for").expect("id");
    insert_live_agent(&registry, &parent, owner.clone());
    insert_live_agent(&registry, &caller, owner.clone());
    insert_child(&registry, &child, owner.clone(), &caller);
    insert_live_agent(&registry, &human, owner.clone());
    insert_live_agent(&registry, &foreign, stranger);

    // The parent: refused, row intact — a child cannot end the session
    // that made it, and this refusal is the scope check doing its work.
    let parent_refusal = registry
        .close_agent_child(&state, &caller, &parent)
        .expect_err("closing its parent is refused");
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&parent),
        "the parent's row survives the refusal"
    );

    // Itself: refused with its own sentence — the caller's MCP client is
    // the process waiting on this reply.
    let self_refusal = registry
        .close_agent_child(&state, &caller, &caller)
        .expect_err("closing itself is refused");
    assert_eq!(
        self_refusal.code,
        ErrorCode::InvalidRequest,
        "{self_refusal:?}"
    );
    assert!(
        self_refusal.message.contains("not its own child"),
        "{}",
        self_refusal.message
    );
    assert!(
        registry
            .inner
            .lock()
            .expect("registry")
            .contains_key(&caller),
        "the caller's row survives the self-refusal"
    );

    // Everything that is not the caller's own live child is one refusal:
    // an invented id, the parent, a human-started session of the same
    // user, a stranger's session. Same code, same sentence — the only
    // difference is the target the caller itself named, so none of the
    // four is distinguishable from the others and existence does not leak.
    let refused_as_not_child = |label: &str, refusal: &WireError, target: &str| {
        assert_eq!(
            refusal.code,
            ErrorCode::SessionNotFound,
            "{label}: {refusal:?}"
        );
        assert_eq!(
            refusal.message,
            format!("none of your live children is called '{target}'"),
            "{label} reads as not-a-child, never as existing-or-not"
        );
    };
    let human_refusal = registry
        .close_agent_child(&state, &caller, &human)
        .expect_err("a session it did not create is refused");
    let foreign_refusal = registry
        .close_agent_child(&state, &caller, &foreign)
        .expect_err("a stranger's session is refused");
    let invented = registry
        .close_agent_child(&state, &caller, "end-invented")
        .expect_err("an invented id is refused");
    refused_as_not_child("the parent", &parent_refusal, &parent);
    refused_as_not_child("a human-started session", &human_refusal, &human);
    refused_as_not_child("a stranger's session", &foreign_refusal, &foreign);
    refused_as_not_child("an invented id", &invented, "end-invented");

    // The green path: the caller's own child goes, and only it does.
    registry
        .close_agent_child(&state, &caller, &child)
        .expect("the caller closes its own child");
    {
        let map = registry.inner.lock().expect("registry");
        assert!(!map.contains_key(&child), "the child's row is gone");
        assert!(map.contains_key(&caller), "the caller's row stays");
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stop_refuses_what_close_refuses_and_preserves_the_row_it_stops() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("stop-children-user", "stop-children-client");
    let parent = compose_session_id(&owner.session_token(), "stop-par").expect("id");
    let caller = compose_session_id(&owner.session_token(), "stop-cal").expect("id");
    let child = compose_session_id(&owner.session_token(), "stop-chi").expect("id");
    insert_live_agent(&registry, &parent, owner.clone());
    insert_live_agent(&registry, &caller, owner.clone());
    insert_child(&registry, &child, owner.clone(), &caller);

    let parent_refusal = registry
        .stop_agent_child(&caller, &parent)
        .expect_err("stopping its parent is refused");
    let invented = registry
        .stop_agent_child(&caller, "stop-invented")
        .expect_err("an invented id is refused");
    let self_refusal = registry
        .stop_agent_child(&caller, &caller)
        .expect_err("stopping itself is refused");
    assert_eq!(
        self_refusal.code,
        ErrorCode::InvalidRequest,
        "{self_refusal:?}"
    );
    assert!(self_refusal.message.contains("not its own child"));
    assert_eq!(parent_refusal.code, ErrorCode::SessionNotFound);
    assert_eq!(
        parent_refusal.message,
        format!("none of your live children is called '{parent}'")
    );
    assert_eq!(
        invented.message,
        "none of your live children is called 'stop-invented'"
    );

    // The green path: the process side dies, the session row stays and is
    // marked preserved, so the transcript survives the stop.
    registry
        .stop_agent_child(&caller, &child)
        .expect("the caller stops its own child");
    {
        let map = registry.inner.lock().expect("registry");
        let live = map
            .get(&child)
            .and_then(RegistryEntry::as_peer_visible)
            .expect("the child's row stays");
        assert!(live.preserve_on_exit.load(Ordering::Acquire));
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stop reaches the whole tree, not just the root: the fixture killer
/// is a no-op, so a grandchild that dies was killed by the child's job
/// object — the same object the spawn path assigns the provider's process
/// to. The row and its transcript survive; only the tree goes.
#[cfg(windows)]
#[test]
fn an_agent_stops_its_own_child_and_the_job_ends_the_tree() {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("stop-tree-user", "stop-tree-client");
    let parent = compose_session_id(&owner.session_token(), "tree-par").expect("id");
    let caller = compose_session_id(&owner.session_token(), "tree-cal").expect("id");
    let child = compose_session_id(&owner.session_token(), "tree-chi").expect("id");
    insert_live_agent(&registry, &parent, owner.clone());
    insert_live_agent(&registry, &caller, owner.clone());
    insert_child(&registry, &child, owner.clone(), &caller);

    let job = {
        let map = registry.inner.lock().expect("registry");
        map.get(&child)
            .and_then(RegistryEntry::as_child_process)
            .map(|session| std::sync::Arc::clone(&session.process_job))
            .expect("the child holds a job object")
    };
    let mut grandchild = std::process::Command::new("ping")
        .args(["-n", "30", "127.0.0.1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("a long-lived grandchild");
    let grandchild_handle = AsRawHandle::as_raw_handle(&grandchild);
    job.assign(grandchild_handle)
        .expect("the grandchild joins the child's job");
    assert!(
        grandchild.try_wait().expect("poll").is_none(),
        "the grandchild is alive before the stop"
    );

    registry
        .stop_agent_child(&caller, &child)
        .expect("the caller stops its own child");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if grandchild.try_wait().expect("poll").is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the grandchild outlived the stop"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    {
        let map = registry.inner.lock().expect("registry");
        let live = map
            .get(&child)
            .and_then(RegistryEntry::as_peer_visible)
            .expect("the child's row stays");
        assert!(live.preserve_on_exit.load(Ordering::Acquire));
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
