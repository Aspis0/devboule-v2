use super::*;

#[test]
fn other_users_sessions_are_counted_without_disclosing_their_titles() {
    let caller = owner();
    let other = OwnerId::new("other-user", "other-client").expect("owner");
    let sessions = vec![
        (
            "a".to_string(),
            caller.clone(),
            "my visible session".to_string(),
        ),
        (
            "b".to_string(),
            other.clone(),
            "private session title".to_string(),
        ),
        ("c".to_string(), other, "another private title".to_string()),
    ];
    let description = super::super::super::describe_sessions(&sessions, &caller);
    assert!(description.contains("and 2 sessions of other users"));
    assert!(description.contains("my visible session"));
    assert!(!description.contains("private session title"));
    assert!(!description.contains("another private title"));
}

#[test]
fn already_ended_session_ids_are_reported_as_closed() {
    let (state, _project, _own, _dir) = setup("archive-ended-race");
    crate::session::insert_test_transcript(&state.sessions, "archive-ended-race-session", owner());
    let result = super::super::super::close_sessions(
        &state,
        vec![(
            "archive-ended-race-session".to_string(),
            owner(),
            "title".to_string(),
        )],
        &mut || {},
    )
    .expect("already ended session is benign");
    assert_eq!(result.closed_session_ids, ["archive-ended-race-session"]);
}
