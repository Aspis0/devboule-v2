//! The signal one page's events bump, and the promise that it is dropped with
//! the tab. No page is opened here: what is tested is the bookkeeping that
//! decides whether a settle waits on a signal that will ever move.

use super::*;

#[test]
fn a_page_that_has_never_been_watched_has_not_moved() {
    assert_eq!(moved("tab-never-seen"), 0);
}

#[test]
fn a_watched_page_has_a_signal_and_forgetting_it_takes_the_signal_with_it() {
    signal_for("tab-1");
    assert_eq!(moved("tab-1"), 0, "a new page starts where it was");

    signal_for("tab-1");
    assert_eq!(moved("tab-1"), 0, "watching a page twice does not move it");

    forget("tab-1");
    assert_eq!(moved("tab-1"), 0, "a closed tab leaves nothing behind");
    forget("tab-never-existed");
}

#[test]
fn the_quiet_a_settle_waits_for_is_shorter_than_the_cap_it_gives_up_at() {
    assert!(QUIET < SETTLE_CAP);
    assert_eq!(
        QUIET,
        std::time::Duration::from_millis(300),
        "a page that has not moved for a third of a second has settled"
    );
}
