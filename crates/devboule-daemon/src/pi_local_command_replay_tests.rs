//! The restart's view of a run pi ended locally: the finish the daemon
//! authored has to be in the journal, or a replayed transcript shows a
//! completed command with no completion.

use super::local_command_test_support::{
    feed, goal_list_notify, idle_state, prompt_response, write_prompt, LocalPi,
};
use crate::journal::{new_session_record, Journal};
use crate::session::SessionRuntime;
use devboule_protocol::{SessionEvent, SessionKind};
use std::sync::Arc;

#[test]
fn a_restarted_replay_derives_the_local_command_finish() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(Journal::open(&path).expect("open"));
    let session_id = "s.pi.local.replay";
    journal
        .create_session(new_session_record(
            session_id,
            "owner",
            None,
            SessionKind::Pi,
            "pi local",
        ))
        .expect("birth");
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    runtime.begin_turn();

    let mut pi = LocalPi::spawn(idle_state());
    let mut reader = pi.reader();
    write_prompt(&pi, "/goal-list");
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, goal_list_notify());
    pi.drain(&mut reader, &runtime, 1);
    feed(&mut reader, &runtime, prompt_response("p-1"));
    pi.drain(&mut reader, &runtime, 2);

    journal.flush().expect("flush");
    drop(runtime);
    drop(reader);
    drop(pi);
    journal.shutdown();

    // The restart: a fresh journal over the same files replays the run.
    let reopened = Journal::open(&path).expect("reopen");
    let replay = reopened.replay(session_id).expect("replay");
    let finish_stop_reasons = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        finish_stop_reasons,
        ["completed"],
        "the restart's replay shows the run completed: {finish_stop_reasons:?}"
    );
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
