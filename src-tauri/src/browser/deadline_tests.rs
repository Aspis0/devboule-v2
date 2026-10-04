use super::*;

#[test]
fn a_wait_cannot_use_the_time_its_own_answer_needs() {
    // The contract lets a caller ask for 12 s and the daemon's budget is 15 s;
    // a command that waited for all of it and then read two trees would be cut
    // off instead of answered.
    let deadline = Deadline::from_now();
    assert!(
        deadline.wait_for() + ANSWER_RESERVE <= COMMAND_BUDGET,
        "a wait plus its answer stays inside the budget: {:?} + {ANSWER_RESERVE:?} <= {COMMAND_BUDGET:?}",
        deadline.wait_for()
    );
    assert!(
        deadline.wait_for() < Duration::from_millis(12_000),
        "and a caller who asks for 12 s is served less rather than timed out"
    );

    // A budget already spent leaves nothing to wait for, and not a negative.
    let spent = Deadline::in_(Duration::from_secs(1));
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(spent.wait_for(), Duration::ZERO);
}

#[test]
fn what_is_left_is_the_whole_budget_less_what_has_passed_and_never_negative() {
    let fresh = Deadline::from_now();
    assert!(fresh.left() <= COMMAND_BUDGET);
    assert!(fresh.left() > COMMAND_BUDGET - Duration::from_secs(1));

    let spent = Deadline::in_(Duration::from_millis(5));
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(spent.left(), Duration::ZERO);
}
