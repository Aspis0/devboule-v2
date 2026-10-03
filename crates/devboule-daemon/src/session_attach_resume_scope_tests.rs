//! What the resume answer must leave as it was: the reply a connection that
//! never negotiated the capability gets, the stored manifest that crosses once,
//! and the shared queue's attach snapshot that crosses once per attach and not
//! once per reset.
//!
//! These are the neighbours of the outcome, not the outcome: each of them is a
//! contract that already existed, and a reset must not move any of them.

use devboule_protocol::{
    caps, intersect_capabilities, m3a_client_capabilities, m3a_daemon_capabilities, Cursor,
    ErrorCode, SessionEvent,
};

use super::session_attach_resume_fixtures::{answer, AttachFixture};

fn fixture() -> AttachFixture {
    AttachFixture::new("process-attach-resume-scope")
}

/// Whether the app's own hello would negotiate `capability` with a daemon: the
/// intersection `server::connection` computes from the two lists.
fn app_negotiates(capability: &str) -> bool {
    intersect_capabilities(&m3a_client_capabilities(), &m3a_daemon_capabilities())
        .iter()
        .any(|cap| cap.as_str() == capability)
}

#[test]
fn a_connection_without_the_capability_keeps_the_mismatch_error_and_a_bare_reply() {
    let f = fixture();
    f.runtime.test_publish_journaled(answer("one"));
    f.journal.flush().expect("flush");
    // The app's own capability list decides this, so the test fails the day the
    // app lane starts negotiating a name whose outcome it cannot read.
    let conn = f.conn(5, app_negotiates(caps::SESSION_RESUME_OUTCOMES));

    let stale = f.registry.attach_with_subscription(
        &f.id,
        1,
        Some(Cursor {
            generation: 99,
            seq: 3,
        }),
        &conn,
        &f.owner,
        false,
    );
    let error = stale.expect_err("an unnegotiated connection must still refuse");
    assert_eq!(
        error.code,
        ErrorCode::SessionGenerationMismatch,
        "without the capability the attach reply is the one it always was"
    );

    let same = f
        .registry
        .attach_with_subscription(
            &f.id,
            2,
            Some(Cursor {
                generation: 1,
                seq: 1,
            }),
            &conn,
            &f.owner,
            false,
        )
        .expect("attach");
    assert!(
        same.is_none(),
        "nothing is added to the reply, so its bytes do not move"
    );

    f.shutdown();
}

#[test]
fn a_reset_attach_still_emits_the_stored_manifest_once() {
    let f = fixture();
    f.runtime
        .test_publish_journaled(answer("before the manifest"));
    f.journal.flush().expect("flush");
    let manifest = SessionEvent::SessionManifest {
        provider_id: Some("claude".into()),
        current_model_id: Some("model-x".into()),
        models: Vec::new(),
        modes: None,
    };
    f.runtime.store_session_manifest(manifest.clone());
    let conn = f.conn(8, true);

    f.registry
        .attach_with_subscription(
            &f.id,
            1,
            Some(Cursor {
                generation: 0,
                seq: 0,
            }),
            &conn,
            &f.owner,
            false,
        )
        .expect("attach");

    let delivered = f.drain(&conn);
    let manifests = delivered
        .iter()
        .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
        .count();
    assert_eq!(
        manifests, 1,
        "the stored manifest replaces history at the seam exactly once: {delivered:?}"
    );

    f.shutdown();
}

/// One connection per attach, because the snapshot is published to every
/// observer on purpose: what this counts is how many the subscriber that just
/// attached receives.
#[test]
fn the_queue_snapshot_crosses_once_per_attach_and_not_once_per_reset() {
    let f = fixture();

    for (index, conn_id) in [9u64, 10, 11].into_iter().enumerate() {
        let conn = f.conn(conn_id, true);
        f.registry
            .attach_with_subscription(
                &f.id,
                index as u64 + 1,
                Some(Cursor {
                    generation: 0,
                    seq: 0,
                }),
                &conn,
                &f.owner,
                false,
            )
            .expect("attach");
        let delivered = f.drain(&conn);
        assert_eq!(
            delivered
                .iter()
                .filter(|event| matches!(event, SessionEvent::QueueSnapshot { .. }))
                .count(),
            1,
            "one attach snapshot per attach, reset or not: {delivered:?}"
        );
    }

    f.shutdown();
}
