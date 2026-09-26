//! The workspace road, moved whole out of `session_tests.rs` lines 2072-2599:
//! the spawn error's workspace id and display path, the local workspace's cwd
//! and the cache in front of it (a hit with the journal gone, the bound, the
//! invalidation), the resume road's created-at and record-own kind, the ACP
//! override refusal, and the delete rules for a local workspace, a worktree and
//! a project folder already gone. Every line below is byte-identical to its
//! text there apart from this header; `tmp_delete_registry` and `test_owner`
//! stay in the provider and are imported here.

use super::tests::{test_owner, tmp_delete_registry};
use super::*;

#[test]
#[cfg(windows)]
fn workspace_spawn_directory_error_names_workspace_in_plain_spelling() {
    let parent = crate::test_dirs::test_temp_dir("devboule-missing-cwd");
    let path = parent.join("Project With Spaces");
    let error = std::process::Command::new("cmd.exe")
        .current_dir(&path)
        .spawn()
        .expect_err("CreateProcess must reject the missing cwd");
    let wire = workspace_spawn_error(Some("w.race"), &path, error);
    assert_eq!(wire.code, ErrorCode::WorkspaceUnavailable);
    assert!(wire.message.contains("w.race"));
    assert!(wire.message.contains("Project With Spaces"));
    assert!(!wire.message.contains(r"\\?\"));
    let _ = std::fs::remove_dir_all(parent);
}

#[test]
fn a_real_local_workspace_supplies_the_session_command_cwd() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("Project With Spaces");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("persist workspace");

    let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    registry
        .apply_workspace_cwd(Some(&workspace.id), &mut command)
        .expect("workspace cwd");
    // The cwd a child receives is the plain spelling of the same folder;
    // storage keeps the verbatim form.
    assert_eq!(
        command.cwd,
        crate::workspace::plain_path(
            &project_path
                .canonicalize()
                .expect("canonical cwd")
                .to_string_lossy()
        )
    );
    assert!(!command.cwd.to_string_lossy().contains("\\\\?\\"));

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unknown_workspace_fails_without_using_the_daemon_cwd() {
    let (dir, registry, journal) = tmp_delete_registry();
    let daemon_cwd = dir.clone();
    let mut command = PtyCommand::new("cmd.exe", Vec::new(), daemon_cwd.clone(), Vec::new());
    let error = registry
        .apply_workspace_cwd(Some("w.missing"), &mut command)
        .expect_err("unknown workspace must fail");
    assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
    assert!(error.message.contains("w.missing"));
    assert_eq!(command.cwd, daemon_cwd);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn workspace_cwd_cache_avoids_a_journal_rpc_after_first_lookup() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("cached-project");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("persist workspace");

    let mut first = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    registry
        .apply_workspace_cwd(Some(&workspace.id), &mut first)
        .expect("first workspace lookup");
    journal.shutdown();

    let mut cached = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    registry
        .apply_workspace_cwd(Some(&workspace.id), &mut cached)
        .expect("cached workspace lookup");
    assert_eq!(
        cached.cwd,
        crate::workspace::plain_path(
            &project_path
                .canonicalize()
                .expect("canonical path")
                .to_string_lossy()
        )
    );
    // This second call succeeds with the journal already shut down, so
    // it proves the hit did not enqueue another workspace RPC.
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_session_against_a_real_local_workspace_echoes_cwd_in_display_form() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("Project With Spaces");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("persist workspace");

    let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    registry
        .apply_workspace_cwd(Some(&workspace.id), &mut command)
        .expect("workspace cwd");
    // The spawn sites echo this exact value onto Session.cwd. A real
    // process is not required to observe the echo: command.cwd is final
    // once apply_workspace_cwd has run.
    let cwd = Some(crate::workspace::plain_path(&command.cwd.to_string_lossy()));
    let expected = crate::workspace::plain_path(
        project_path
            .canonicalize()
            .expect("canonical cwd")
            .to_str()
            .expect("canonical cwd is valid UTF-8"),
    );
    assert_eq!(cwd.as_deref(), Some(expected.as_str()));
    assert!(
        !cwd.as_deref().expect("cwd echo").starts_with(r"\\?\"),
        "wire cwd must not carry the verbatim prefix: {cwd:?}"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn journal_only_transcript_session_does_not_invent_a_cwd() {
    let record = new_session_record(
        "s.client.1",
        "S-1-5-21-1",
        Some("w.1".to_string()),
        SessionKind::Terminal,
        "Terminal",
    );
    let session = record.to_session();
    assert_eq!(session.workspace_id.as_deref(), Some("w.1"));
    assert_eq!(
        session.cwd, None,
        "journal rows have no cwd column; None means unknown, not a guessed workspace path"
    );
    assert_eq!(session.created_at_ms, record.created_at_ms);
}

#[test]
fn resume_preserves_the_original_created_at_ms() {
    let mut record = new_session_record(
        "s.client.1",
        "S-1-5-21-1",
        Some("w.1".to_string()),
        SessionKind::Acp,
        "Agent",
    );
    record.created_at_ms = 1_700_000_000_123;
    let command = PtyCommand::new(
        "cmd.exe",
        Vec::new(),
        crate::test_dirs::test_temp_dir("devboule-pty-cwd"),
        Vec::new(),
    );
    let session = session_metadata_for_resume(
        "s.client.1",
        &record,
        &command,
        "grok".to_string(),
        "peer-1".to_string(),
        2,
    );
    assert_eq!(session.created_at_ms, 1_700_000_000_123);
    assert_eq!(session.id, "s.client.1");
    assert_eq!(session.state, SessionState::Live { generation: 2 });
    assert!(
        !session.resumable,
        "a just-resumed live row never offers resume"
    );
}

#[test]
fn resume_metadata_kind_is_the_records_own_kind_not_the_provider_string() {
    // Replaces `resume_metadata_kind_follows_the_resolved_provider`, and the
    // reason is a MAX RECALL finding, so it is written down rather than
    // quietly swapped.
    //
    // Pass 2c derived the resumed kind from the provider string so a future
    // family's resume would not be reported as ACP. The audit showed the
    // premise was false: `provider` is a string on a row that CAN disagree
    // with its own kind. `DEVBOULE_ACP_PROVIDER_ID` reaches
    // `command.provider_id` without passing the native-id strip, so a create
    // through the ACP command override journals `kind=acp, provider=codex`,
    // and deriving from the provider stamped `Codex` on a session whose peer
    // is ACP — which `start_spawned_session` then installs on the runtime,
    // weakening the MCP gate and routing ACP envelopes through the Codex view.
    //
    // So the fixture below is the DANGEROUS pair on purpose: the record says
    // `Acp`, the provider says a native family. The record wins. The old test
    // asserted the opposite on this very pair, and that is the decision this
    // one reverses.
    let command = PtyCommand::new(
        "cmd.exe",
        Vec::new(),
        crate::test_dirs::test_temp_dir("devboule-pty-cwd"),
        Vec::new(),
    );
    for provider_id in ["grok", "claude", "codex", "pi"] {
        let record = new_session_record(
            "s.client.1",
            "S-1-5-21-1",
            Some("w.1".to_string()),
            SessionKind::Acp,
            "Agent",
        );
        let session = session_metadata_for_resume(
            "s.client.1",
            &record,
            &command,
            provider_id.to_string(),
            "peer-1".to_string(),
            2,
        );
        assert_eq!(
            session.kind,
            SessionKind::Acp,
            "an ACP row stays ACP however its provider string reads ({provider_id})"
        );
    }

    // And it is not a constant: a row of another kind stamps that kind, which
    // is what pass 2c wanted and what the provider lookup was reaching for.
    let record = new_session_record(
        "s.client.2",
        "S-1-5-21-1",
        Some("w.1".to_string()),
        SessionKind::Pi,
        "Agent",
    );
    let session = session_metadata_for_resume(
        "s.client.2",
        &record,
        &command,
        "pi".to_string(),
        "peer-1".to_string(),
        2,
    );
    assert_eq!(session.kind, SessionKind::Pi);
}

/// The write-side twin of the test above: the `kind=acp, provider=codex` row
/// that test survives being *read* must never be *born*. The one road that
/// writes it is `DEVBOULE_ACP_PROVIDER_ID`, which reaches the row's provider
/// without passing the registry — the override tests all named
/// `devboule-acp-stub`, which is why the suite never saw the poison. The id
/// here is `codex`, the native family the MAX RECALL audit caught: `claude`
/// and `pi` are stripped from a create's own provider field, and a requested
/// `codex` remaps the whole create to the Codex family, so the env road is
/// the only way a native id reaches the stamp untouched.
#[test]
fn the_acp_command_override_cannot_journal_a_native_provider_id() {
    let state = ServerState::new("acp-native-id-strip".to_string());
    let owner = test_owner("S-1-5-21-acp-strip", "acp-native-strip");
    let _acp_env = crate::session::lock_acp_env();
    std::env::set_var(
        "DEVBOULE_ACP_COMMAND",
        r#"["definitely-not-a-real-program-xyz"]"#,
    );
    std::env::set_var("DEVBOULE_ACP_PROVIDER_ID", "codex");
    let created = state.sessions.create_with_provider_env(
        &state,
        &owner,
        None,
        SessionKind::Acp,
        None,
        crate::profile_delivery::ProfileDelivery::none(),
        None,
        &None,
        None,
        &SessionCreateMeta::default(),
    );
    std::env::remove_var("DEVBOULE_ACP_COMMAND");
    std::env::remove_var("DEVBOULE_ACP_PROVIDER_ID");
    // Whatever the create answered, no row it may have written may name one
    // family in its kind and another in its provider. The family a provider
    // string names is the create road's own derivation
    // (`session_kind_for`), deliberately not the guard's own enumeration.
    let rows = state
        .sessions
        .journal
        .as_ref()
        .expect("the test state has a journal")
        .list()
        .expect("journal rows");
    for row in rows
        .iter()
        .filter(|row| matches!(row.kind, SessionKind::Acp))
    {
        let named_family = row
            .provider
            .as_deref()
            .map(crate::provider_catalog::session_kind_for);
        assert_eq!(
            named_family,
            Some(row.kind.clone()),
            "row {} journals kind {:?} beside provider {:?}: the write side accepted \
             the pair the read side was fixed to survive",
            row.id,
            row.kind,
            row.provider
        );
    }
    // And when the create is refused, the refusal names the override id —
    // not the spawn accident this fixture's command would otherwise die of.
    let error = created.expect_err("the override create must not journal a native id");
    assert!(
        error.message.contains("DEVBOULE_ACP_PROVIDER_ID"),
        "the refusal names the override id, not a downstream failure: {error:?}"
    );
}

#[test]
fn workspace_lookup_reports_journal_failure_not_a_missing_workspace() {
    let (dir, registry, journal) = tmp_delete_registry();
    journal.shutdown();
    let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    let error = registry
        .apply_workspace_cwd(Some("w.journal-stopped"), &mut command)
        .expect_err("stopped journal must fail");
    assert_eq!(error.code, ErrorCode::Journal);
    assert!(error.message.contains("journal writer has stopped"));
    assert!(!error.message.contains("does not exist"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn workspace_path_cache_evicts_old_entries_at_its_bound() {
    let mut cache = WorkspacePathCache::default();
    for index in 0..=WORKSPACE_PATH_CACHE_CAP {
        cache.insert(
            format!("w.{index}"),
            PathBuf::from(format!("C:\\workspace-{index}")),
        );
    }
    assert_eq!(cache.entries.len(), WORKSPACE_PATH_CACHE_CAP);
    assert!(cache.get("w.0").is_none());
    assert!(cache
        .get(&format!("w.{WORKSPACE_PATH_CACHE_CAP}"))
        .is_some());
}

#[test]
fn workspace_cache_invalidation_reports_a_missing_folder_not_a_deadline() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("missing-project");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::local_workspace_record(&project))
        .expect("persist workspace");
    let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    registry
        .apply_workspace_cwd(Some(&workspace.id), &mut command)
        .expect("cache workspace");
    std::fs::remove_dir_all(&project_path).expect("remove workspace folder");

    let mut missing = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    let error = registry
        .apply_workspace_cwd(Some(&workspace.id), &mut missing)
        .expect_err("missing workspace folder");
    assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
    assert!(error.message.contains("folder is no longer available"));
    assert!(!error.message.contains("deadline"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn workspace_create_rejects_a_branch_on_a_local_workspace() {
    let (dir, registry, journal) = tmp_delete_registry();
    let error = registry
        .workspace_create(
            "p.not-needed-for-branch-rejection",
            WorkspaceIsolation::Local,
            Some("feature-x".to_string()),
        )
        .expect_err("branch must not be silently ignored");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("branch"));
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn worktree_create_refuses_when_live_git_is_not_a_repository() {
    let error = refuse_worktree_unless_live_git_allows("repository", "not_repository", "p.one")
        .expect_err("live not_repository must refuse even if recorded says repository");
    assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
    match error.details {
        Some(devboule_protocol::ErrorDetails::WorktreeGitState { recorded, observed }) => {
            assert_eq!(recorded, "repository");
            assert_eq!(observed, "not_repository");
        }
        other => panic!("expected WorktreeGitState, got {other:?}"),
    }
    assert!(
        refuse_worktree_unless_live_git_allows("not_repository", "repository", "p.one").is_ok(),
        "live repository must win over a stale recorded not_repository"
    );
}

#[test]
fn worktree_workspace_cwd_uses_the_checkout_path_not_the_project() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("project");
    let checkout = dir.join("checkout");
    std::fs::create_dir(&project_path).expect("project folder");
    std::fs::create_dir(&checkout).expect("checkout folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::worktree_workspace_record(
            &project,
            &checkout,
            "feature-x",
        ))
        .expect("persist worktree workspace");
    assert_eq!(workspace.isolation, WorkspaceIsolation::Worktree);
    let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
    registry
        .apply_workspace_cwd(Some(&workspace.id), &mut command)
        .expect("worktree cwd");
    assert_eq!(command.cwd, checkout);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn workspace_delete_detaches_the_row_when_the_project_folder_is_gone() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("project");
    let checkout = dir.join("project.worktrees").join("kept");
    std::fs::create_dir(&project_path).expect("project folder");
    std::fs::create_dir_all(&checkout).expect("checkout");
    std::fs::write(checkout.join("uncommitted.txt"), "keep me").expect("work");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::worktree_workspace_record(
            &project,
            &checkout,
            "feature/a",
        ))
        .expect("persist worktree workspace");
    std::fs::remove_dir_all(&project_path).expect("remove project folder");
    registry
        .workspace_delete(&workspace.id, false)
        .expect("row must be removable when the project folder is gone");
    assert!(
        journal.workspace_get(&workspace.id).expect("get").is_none(),
        "stale row must be detached"
    );
    assert!(
        checkout.join("uncommitted.txt").is_file(),
        "uncommitted work must stay on disk"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn workspace_delete_refuses_a_checkout_outside_the_project_worktree_root() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("project");
    let outsider = dir.join("someone-else");
    std::fs::create_dir(&project_path).expect("project folder");
    std::fs::create_dir(&outsider).expect("outsider");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = journal
        .workspace_create(crate::workspace::worktree_workspace_record(
            &project,
            &outsider,
            "feature/a",
        ))
        .expect("persist worktree workspace");
    let error = registry
        .workspace_delete(&workspace.id, true)
        .expect_err("must not git-remove a path outside the worktree root");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(matches!(
        error.details,
        Some(devboule_protocol::ErrorDetails::WorktreeNotConfined { .. })
    ));
    assert!(outsider.is_dir(), "outsider checkout must be untouched");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn local_workspace_delete_does_not_remove_the_project_folder() {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("project");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let workspace = registry
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("local workspace");
    let error = registry
        .workspace_delete(&workspace.id, false)
        .expect_err("local workspace must not be deleted as a worktree");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(project_path.is_dir());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The creation serial, pinned directly: eight barrier-aligned worktree
/// creates on different branches never overlap inside the function, so a
/// loser always sees a winner's finished state when it decides what to
/// clean. Without the lock this observes more than one.
#[test]
fn concurrent_worktree_creates_never_overlap() {
    let (dir, registry, journal) = tmp_delete_registry();
    let root = dir.join("RaceProject");
    std::fs::create_dir_all(&root).expect("project folder");
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    run(&["init"]);
    std::fs::write(root.join("seed.txt"), "seed").expect("seed file");
    run(&["add", "seed.txt"]);
    run(&["commit", "-m", "seed"]);
    let project = registry
        .project_add(root.to_str().expect("project path"))
        .expect("project row");

    registry.worktree_probe.reset();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|index| {
            let registry = registry.clone();
            let project_id = project.id.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                registry.workspace_create(
                    &project_id,
                    WorkspaceIsolation::Worktree,
                    Some(format!("race-branch-{index}")),
                )
            })
        })
        .collect();
    let mut created = 0;
    for handle in handles {
        if handle.join().expect("create thread").is_ok() {
            created += 1;
        }
    }
    assert_eq!(created, 8, "the serial orders creates, it never fails them");
    assert_eq!(
        registry.worktree_probe.max_seen(),
        1,
        "no two creates share the function"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The loser's guard, arm by arm, without threads: a path git does not
/// list is attempted (never kept), and a live checkout on another branch
/// is kept (never removed).
#[test]
fn loser_cleanup_attempts_a_missing_path_and_keeps_a_mismatch() {
    use super::session_workspaces::{loser_checkout_cleanup, LoserCleanup};

    let (dir, registry, journal) = tmp_delete_registry();
    let root = dir.join("GuardProject");
    std::fs::create_dir_all(&root).expect("project folder");
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    run(&["init"]);
    std::fs::write(root.join("seed.txt"), "seed").expect("seed file");
    run(&["add", "seed.txt"]);
    run(&["commit", "-m", "seed"]);
    let project = registry
        .project_add(root.to_str().expect("project path"))
        .expect("project row");

    // Missing: nothing is live there, so the guard removes rather than
    // keeping — on an absent path the removal recovery reports success,
    // since there is nothing to clean.
    assert_eq!(
        loser_checkout_cleanup(&root, &root.join("no-such-checkout"), "b"),
        LoserCleanup::Removed
    );

    // BranchMismatch: a live checkout on another branch is kept, untouched.
    let winner = registry
        .workspace_create(
            &project.id,
            WorkspaceIsolation::Worktree,
            Some("guard-branch".to_string()),
        )
        .expect("winner row");
    let winner_path = std::path::PathBuf::from(&winner.path);
    assert_eq!(
        loser_checkout_cleanup(&root, &winner_path, "other-branch"),
        LoserCleanup::KeptLive
    );
    assert!(winner_path.is_dir(), "a foreign checkout stands");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The killed-add aftermath, repaired exactly: a listed checkout with no
/// journal row (what a `git worktree add` killed mid-registration leaves)
/// is removed and pruned, and the message says what happened.
#[test]
fn a_killed_add_repairs_exactly_its_recorded_debris() {
    use super::session_workspaces::repair_killed_worktree_add;

    let (dir, registry, journal) = tmp_delete_registry();
    let root = dir.join("KillProject");
    std::fs::create_dir_all(&root).expect("project folder");
    let run = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(&root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    run(&["init"]);
    std::fs::write(root.join("seed.txt"), "seed").expect("seed file");
    run(&["add", "seed.txt"]);
    run(&["commit", "-m", "seed"]);
    let project = registry
        .project_add(root.to_str().expect("project path"))
        .expect("project row");

    // The debris: git registered it, the journal never saw it.
    let debris = root.join("debris-checkout");
    run(&[
        "worktree",
        "add",
        "-b",
        "debris-branch",
        debris.to_str().expect("debris path"),
        "HEAD",
    ]);
    assert!(debris.is_dir(), "the debris stands before the repair");

    let message = repair_killed_worktree_add("p.kill", &root, &debris);
    assert!(
        message.contains("removed the partial checkout") && message.contains("pruned"),
        "the repair is reported: {message}"
    );
    assert!(!debris.exists(), "exactly that path is gone");
    let list = std::process::Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(&root)
        .output()
        .expect("git list runs");
    let list = String::from_utf8_lossy(&list.stdout);
    assert!(
        !list.contains("debris"),
        "prune cleared the registration: {list}"
    );
    let _ = project;
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
