use super::*;

#[test]
fn a_tab_starts_parked_with_no_override() {
    let live = Live::default();

    assert!(live.parked());
    assert!(!live.overridden());
}

#[test]
fn taking_the_override_says_whether_there_was_one_and_leaves_none() {
    let live = Live::default();
    live.set_overridden(true);

    assert!(live.take_overridden());
    assert!(!live.overridden());
    assert!(!live.take_overridden(), "a second take finds nothing");
}
