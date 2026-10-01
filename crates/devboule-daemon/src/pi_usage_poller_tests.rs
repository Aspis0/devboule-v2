//! The live-context poller's behaviour: what a tick publishes and skips,
//! every stop road, the refusal-only give-up and the not-a-give-up
//! timeout, compaction nulls, unknown windows, model switches, the
//! in-flight reply that outlives its window, the stall watchdog the poll
//! must not feed, the journal rows the poll may and may not eat, and what
//! replay restores. The clock is the poller's own tick; the fake pi
//! answers from files the tests restage between ticks.

use super::super::pi_turn_watch::test_support::broker;
use super::test_support::{
    context_usages, drain, feed_line, harness, harness_catalog, harness_journaled,
    harness_with_catalog, harness_with_silence, node_skip, prime, release_hold, stage_hold,
    stage_stats, stats_requests, turn_end, unstage_stats, wait_dead, wait_for, wait_for_window,
    wait_held,
};
use crate::journal::{new_session_record, Journal};
use crate::session::SessionKiller;
use crate::test_support::is_watchdog_error;
use devboule_protocol::SessionKind;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

fn stats_body(tokens: impl Into<serde_json::Value>, window: Option<u64>) -> serde_json::Value {
    let mut context = json!({"tokens": tokens.into()});
    if let Some(window) = window {
        context["contextWindow"] = window.into();
    }
    json!({"contextUsage": context})
}

#[test]
fn a_tick_publishes_a_changed_reading_and_skips_an_unchanged_one() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the first reading",
    );
    assert_eq!(
        context_usages(&events),
        [(Some("m".to_string()), 1000, Some(4000), true)],
        "the first reading, live, with the stats window: {events:?}"
    );
    // The same stats again: nothing publishes.
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "an unchanged reading republishes nothing: {quiet:?}"
    );
    // Changed stats: the next tick publishes the new reading.
    stage_stats(&harness, stats_body(1200, Some(4000)));
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the changed reading",
    );
    assert_eq!(
        context_usages(&events),
        [(Some("m".to_string()), 1200, Some(4000), true)],
        "the changed reading: {events:?}"
    );
}

/// One window resolution, end to end: its own harness on the catalog the
/// manifest window names, the body staged, one tick, the reading that
/// published.
fn reading_of(
    window: Option<u64>,
    body: serde_json::Value,
    label: &str,
) -> (Option<String>, u64, Option<u64>, bool) {
    let broker = broker();
    let harness = harness_with_catalog(&broker, harness_catalog(window));
    prime(&harness);
    stage_stats(&harness, body);
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        label,
    );
    let readings = context_usages(&events);
    let [reading] = readings.as_slice() else {
        panic!("one reading for {label}, got {events:?}");
    };
    reading.clone()
}

#[test]
fn the_window_comes_from_the_stats_then_the_manifest_then_nowhere() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    assert_eq!(
        reading_of(
            Some(4000),
            stats_body(1000, Some(12345)),
            "the stats-window reading"
        ),
        (Some("m".to_string()), 1000, Some(12345), true),
        "the stats window is taken as-is"
    );
    assert_eq!(
        reading_of(
            Some(4000),
            stats_body(1000, None),
            "the manifest-window reading"
        ),
        (Some("m".to_string()), 1000, Some(4000), true),
        "no stats window: the manifest window of the same model"
    );
    assert_eq!(
        reading_of(None, stats_body(1000, None), "the unknown-window reading"),
        (Some("m".to_string()), 1000, None, true),
        "no window anywhere: the used tokens stay, the max is absent"
    );
}

#[test]
fn a_closed_run_polls_no_more() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the mid-run reading",
    );
    // The run closes: the durable turn_end reading is the turn's final
    // value, so no completion read is owed and no further request goes
    // out.
    harness.poller.run_closed();
    assert!(
        !harness.poller.window_open_for_test(),
        "the close shuts the window"
    );
    let sent = stats_requests(&harness);
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    assert_eq!(
        stats_requests(&harness),
        sent,
        "a closed run sends no request"
    );
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "a closed run publishes nothing: {quiet:?}"
    );
    // A fresh run polls again.
    stage_stats(&harness, stats_body(1200, Some(4000)));
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the fresh run's reading",
    );
    assert_eq!(
        context_usages(&events),
        [(Some("m".to_string()), 1200, Some(4000), true)],
        "the fresh run polls: {events:?}"
    );
}

#[test]
fn a_reply_in_flight_across_a_close_never_publishes() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    stage_hold(&harness);
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    // One tick, on its own thread, with the fake pi holding the reply —
    // the interleave the synchronous suite could not reach before.
    let ticking = Arc::clone(&harness.poller);
    let tick = std::thread::spawn(move || ticking.tick_for_test());
    wait_held(&harness);
    // The whole run lifecycle passes underneath the read: the turn ends,
    // a replacement begins. The held reply is turn one's count.
    harness.poller.run_closed();
    harness.poller.run_opened();
    release_hold(&harness);
    tick.join().expect("the tick finishes once the reply lands");
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "a reply whose window closed behind it publishes nowhere: {quiet:?}"
    );
    assert!(
        harness.poller.window_open_for_test(),
        "the replacement's window stays open"
    );
    // And it poisons nothing: the fresh window's own answer publishes.
    stage_stats(&harness, stats_body(1200, Some(4000)));
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the replacement's reading",
    );
    assert_eq!(
        context_usages(&events),
        [(Some("m".to_string()), 1200, Some(4000), true)],
        "the fresh reading: {events:?}"
    );
}

#[test]
fn an_interrupt_stops_the_window_and_the_requests() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let mut harness = harness(&broker());
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_opened();
    // The killer's interrupt runs the production road: arbiter, then
    // window. A stopped window sends no request and publishes nothing.
    harness.killer.interrupt();
    assert!(
        !harness.poller.window_open_for_test(),
        "the interrupt stops the window"
    );
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    assert_eq!(
        stats_requests(&harness),
        0,
        "an interrupted run sends no request"
    );
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "an interrupted run reads no last word: {quiet:?}"
    );
}

#[test]
fn a_kill_stops_the_window_and_the_requests() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let mut harness = harness(&broker());
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_opened();
    // The kill road, by its own word: the window stops with the process
    // tree, not with the stdout EOF that follows it.
    harness.killer.kill();
    // The kill terminates the process itself: wait for the death before
    // anything counts what the log holds.
    wait_dead(&harness);
    assert!(
        !harness.poller.window_open_for_test(),
        "the kill stops the window"
    );
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    assert_eq!(
        stats_requests(&harness),
        0,
        "a killed session sends no request"
    );
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "a killed session publishes nothing: {quiet:?}"
    );
}

#[test]
fn a_pi_that_only_answers_stats_still_hits_the_watchdog() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    // A short REAL silence, and a poll loop that keeps asking across it:
    // the defect this test names is a poll reply feeding the stall clock,
    // which only shows while the polls continue. The watch's own thread
    // judges the silence; the replies travel the production reader.
    let harness = harness_with_silence(&broker, Duration::from_millis(400));
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    feed_line(&harness, json!({"type": "agent_start"}));
    wait_for_window(&harness, true, "the run opens the window");
    let stop_polls = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let looping = Arc::clone(&harness.poller);
    let stop = Arc::clone(&stop_polls);
    let loop_thread = std::thread::spawn(move || {
        while !stop.load(std::sync::atomic::Ordering::Acquire) {
            looping.arm_next_poll_now_for_test();
            looping.tick_for_test();
            std::thread::sleep(Duration::from_millis(40));
        }
    });
    // pi answers every stats request and produces no output: the watchdog
    // must still end the run while the polls keep going.
    wait_for(
        &harness.conn,
        |batch| batch.iter().any(is_watchdog_error),
        "the expiry while the polls continue",
    );
    wait_for_window(&harness, false, "the expiry stops the window");
    let sent = stats_requests(&harness);
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        stats_requests(&harness),
        sent,
        "the expiry stops the requests"
    );
    stop_polls.store(true, std::sync::atomic::Ordering::Release);
    loop_thread.join().expect("the poll loop ends");
}

#[test]
fn a_model_switch_mid_turn_stops_the_window_silently() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_opened();
    // The switch the `set_model` rpc lands in the catalog the poller reads.
    if let Ok(mut catalog) = harness.catalog.lock() {
        catalog.current_model_id = Some("other".to_string());
    }
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "no reading publishes under the wrong model: {quiet:?}"
    );
    assert!(
        !harness.poller.window_open_for_test(),
        "the switch closes the window"
    );
    assert_eq!(
        stats_requests(&harness),
        0,
        "the switch stops the requests before any goes out"
    );
}

#[test]
fn a_refused_stats_command_gives_up_for_the_session() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    // Nothing staged: the refusal shape an old binary's unknown command
    // takes. One tick reads it, stops the session's polling, and says so
    // in the daemon log.
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    wait_for_window(&harness, false, "the give-up closes the window");
    let sent = stats_requests(&harness);
    // A working stats RPC appearing later never revives the session's
    // poll: the turn_end reading carries the meter alone.
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    assert_eq!(
        stats_requests(&harness),
        sent,
        "a session given up on is never asked again"
    );
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "a given-up session publishes nothing: {quiet:?}"
    );
    unstage_stats(&harness);
}

#[test]
fn a_timed_out_poll_is_not_a_give_up() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    stage_hold(&harness);
    harness
        .poller
        .set_budget_for_test(Duration::from_millis(150));
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    // The reply is held past the budget: no answer arrived, which is
    // evidence about nothing — no reading this tick, and no latch.
    harness.poller.tick_for_test();
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "a timed-out tick publishes nothing: {quiet:?}"
    );
    // The reply lands once released; the next tick reads again and
    // publishes — a poller latched by the timeout would stay silent.
    release_hold(&harness);
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the reading after the timeout",
    );
    assert_eq!(
        context_usages(&events),
        [(Some("m".to_string()), 1000, Some(4000), true)],
        "the timeout was no give-up: {events:?}"
    );
}

#[test]
fn a_compaction_null_publishes_nothing_and_keeps_polling() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    // Just after compaction pi reports null tokens: publish nothing —
    // never a zero — and keep the window open for the next real answer.
    stage_stats(&harness, stats_body(serde_json::Value::Null, Some(4000)));
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "a null reading publishes nothing: {quiet:?}"
    );
    assert!(
        harness.poller.window_open_for_test(),
        "the null is not a give-up: the poll continues"
    );
    stage_stats(&harness, stats_body(900, Some(4000)));
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the post-compaction reading",
    );
    assert_eq!(
        context_usages(&events),
        [(Some("m".to_string()), 900, Some(4000), true)],
        "the next real answer publishes: {events:?}"
    );
}

#[test]
fn no_poll_without_an_open_run() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "there is no poll outside a turn: {quiet:?}"
    );
}

#[test]
fn a_control_get_state_reply_still_journals() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(Journal::open(&path).expect("open"));
    let session_id = "s.pi.usage.journal".to_string();
    journal
        .create_session(new_session_record(
            &session_id,
            "owner",
            None,
            SessionKind::Pi,
            "pi usage journal",
        ))
        .expect("birth");
    let harness = harness_journaled(&broker(), &journal, &session_id);
    prime(&harness);
    // A control `get_state`, the shape `/autocompact toggle` sends:
    // claimed, `c-`-tagged, answered. The poll's journal exclusion keys
    // on the poller's own `u-` ids, so this row keeps its place.
    let (id, response) = harness
        .control
        .begin("get_state", json!({}))
        .expect("begin the control request");
    feed_line(
        &harness,
        json!({
            "id": id,
            "type": "response",
            "command": "get_state",
            "success": true,
            "data": {"autoCompactionEnabled": true},
        }),
    );
    let answered = response
        .recv_timeout(Duration::from_secs(5))
        .expect("the claimed reply is delivered");
    assert!(
        answered
            .expect("the reply succeeded")
            .get("success")
            .and_then(serde_json::Value::as_bool)
            == Some(true),
        "the control waiter owns the answer"
    );
    drain(&harness.conn);
    drop(harness);
    journal.flush().expect("flush");
    journal.shutdown();

    let conn = rusqlite::Connection::open(&path).expect("inspect");
    let mut statement = conn
        .prepare("SELECT payload FROM events WHERE session_id = ?1 AND kind = 'acp_envelope'")
        .expect("prepare");
    let payloads: Vec<Vec<u8>> = statement
        .query_map([&session_id], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    // The request frame the fake echoes back is a journal row too; the
    // reply is the row the exclusion must not eat.
    let hits = payloads
        .iter()
        .filter(|payload| {
            let row: serde_json::Value = serde_json::from_slice(payload).expect("envelope payload");
            row.get("id").and_then(serde_json::Value::as_str) == Some(id.as_str())
                && row.get("type").and_then(serde_json::Value::as_str) == Some("response")
        })
        .count();
    assert_eq!(
        hits,
        1,
        "the control get_state reply journals exactly once across {} envelope rows",
        payloads.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_durable_turn_end_reading_resets_the_dedup() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the first turn's live reading",
    );
    // The durable reading lands at the turn's end — a different number —
    // and becomes what the client last saw.
    feed_line(&harness, turn_end(25_851, "stop"));
    wait_for(
        &harness.conn,
        |batch| context_usages(batch).iter().any(|reading| !reading.3),
        "the durable reading",
    );
    // The next turn's first live reading EQUALS the last live one. The
    // dedup key claims to track what the client last saw — the durable
    // number, not the old live one — so it must publish.
    stage_stats(&harness, stats_body(1000, Some(4000)));
    harness.poller.run_closed();
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    let events = wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the equal reading the reset lets through",
    );
    assert_eq!(
        context_usages(&events),
        [(Some("m".to_string()), 1000, Some(4000), true)],
        "the durable reading reset the key: {events:?}"
    );
}

#[test]
fn live_readings_are_not_journalled_and_replay_restores_the_turn_end_reading() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(Journal::open(&path).expect("open"));
    let session_id = "s.pi.usage.replay".to_string();
    journal
        .create_session(new_session_record(
            &session_id,
            "owner",
            None,
            SessionKind::Pi,
            "pi usage replay",
        ))
        .expect("birth");
    let harness = harness_journaled(&broker(), &journal, &session_id);
    stage_stats(&harness, stats_body(3000, Some(4000)));
    // The run wiring, through the production reader: `agent_start` opens
    // the window, a mid-run tick publishes live, the `turn_end` publishes
    // the durable reading, and `agent_end` closes the window with nothing
    // owed.
    feed_line(&harness, json!({"type": "agent_start"}));
    wait_for_window(&harness, true, "the run opens the window");
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    wait_for(
        &harness.conn,
        |batch| !context_usages(batch).is_empty(),
        "the mid-run live reading",
    );
    feed_line(&harness, turn_end(25_851, "stop"));
    wait_for(
        &harness.conn,
        |batch| context_usages(batch).iter().any(|reading| !reading.3),
        "the turn_end reading",
    );
    feed_line(&harness, json!({"type": "agent_end"}));
    wait_for_window(&harness, false, "the run closes the window");
    let sent = stats_requests(&harness);
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    assert_eq!(
        stats_requests(&harness),
        sent,
        "no completion read: the turn_end reading is the final value"
    );
    drain(&harness.conn);
    drop(harness);
    journal.flush().expect("flush");
    journal.shutdown();

    let replay = Journal::open(&path)
        .expect("reopen")
        .replay(&session_id)
        .expect("replay");
    assert_eq!(
        context_usages(&replay.events),
        [(Some("m".to_string()), 25_851, None, false)],
        "replay restores the turn_end reading alone: {:?}",
        context_usages(&replay.events)
    );
    // The stats replies left no row at all: the poll's cadence must not
    // bloat the journal, whether or not a row would have derived an event.
    let conn = rusqlite::Connection::open(&path).expect("inspect");
    let mut statement = conn
        .prepare("SELECT payload FROM events WHERE session_id = ?1 AND kind = 'acp_envelope'")
        .expect("prepare");
    let payloads: Vec<Vec<u8>> = statement
        .query_map([&session_id], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    assert!(
        payloads
            .iter()
            .all(|payload| !String::from_utf8_lossy(payload).contains("get_session_stats")),
        "the poll's replies never became rows across {} envelope rows",
        payloads.len()
    );
    let _ = std::fs::remove_dir_all(&dir);
}
