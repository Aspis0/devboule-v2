//! The macOS probe's pure parsers on fixed input: the calendar arithmetic
//! behind a creation time, and the `ps` and `lsof` record shapes.

use super::*;

/// Day counts against known calendar dates, either side of the epoch and
/// across a leap day: a creation time is only as right as these.
#[test]
fn days_from_civil_matches_known_dates() {
    assert_eq!(days_from_civil(1970, 1, 1), 0);
    assert_eq!(days_from_civil(1969, 12, 31), -1);
    assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    assert_eq!(days_from_civil(2024, 2, 29), 19_782);
    assert_eq!(days_from_civil(2026, 10, 5), 20_731);
}

#[test]
fn a_ps_listing_keeps_pid_parent_group_and_command() {
    let rows = parse_ps_rows(
        "  12   1  12 /bin/sh -c sleep 5
not a row
",
    );
    let row = rows.get(&12).expect("the numeric row is kept");
    assert_eq!((row.ppid, row.pgid), (1, 12));
    assert_eq!(row.command, "/bin/sh -c sleep 5");
    assert_eq!(rows.len(), 1, "a line without three numbers proves nothing");
}

/// `ps` pads its numeric columns, so real lines open with spaces and have
/// runs of them between columns.
#[test]
fn right_aligned_ps_columns_still_parse() {
    let rows = parse_ps_rows(
        "    1     0     1 /sbin/launchd
 4242   318  4242 node  server.js --port 80
",
    );
    assert_eq!(
        rows.get(&1).map(|row| row.command.as_str()),
        Some("/sbin/launchd")
    );
    let row = rows.get(&4242).expect("a padded row is kept");
    assert_eq!((row.ppid, row.pgid), (318, 4242));
    assert_eq!(row.command, "node  server.js --port 80");
}

#[test]
fn lsof_records_pair_each_port_with_the_pid_line_above_it() {
    let ports = parse_lsof_ports(
        "p42
n*:8080
n[::1]:9000
p7
n127.0.0.1:53
",
    );
    assert_eq!(ports, vec![(8080, 42), (9000, 42), (53, 7)]);
}

/// Real BSD `ps -o stat=` output: a running or sleeping process is not exited,
/// a zombie (killed, parent has not reaped it) and an unlisted pid are.
#[test]
fn a_zombie_or_an_unlisted_pid_has_exited_and_a_live_one_has_not() {
    for live in [
        "Ss  
", "S+
", "R
", "  Ss
", "U
", "SNs
",
    ] {
        assert!(!stat_is_exited(live), "{live:?} is a live process");
    }
    for gone in [
        "Z
", "Z+
", "  Z 
", "Zs
", "", "
", "   
",
    ] {
        assert!(stat_is_exited(gone), "{gone:?} is exited");
    }
}

/// Real BSD `ps -axo pid=,lstart=` lines: a padded pid column, a space-padded
/// day, and a line that is not a pair. Compared against each other, so the
/// zone the host runs in cannot change the result.
#[test]
fn lstart_lines_with_padded_columns_become_start_times() {
    let mut rows = parse_ps_rows(
        "    1     0     1 /sbin/launchd
 4242   318  4242 sleep 1
 4300 318 4300 sh
",
    );
    apply_start_times(
        &mut rows,
        "    1 Mon Oct  6 06:12:01 2026
 4242 Mon Oct  6 06:12:02 2026
 4300 not a stamp
 nonsense
",
    );
    let first = rows
        .get(&1)
        .and_then(|row| row.started_at_ticks)
        .expect("padded pid parses");
    let second = rows
        .get(&4242)
        .and_then(|row| row.started_at_ticks)
        .expect("padded day parses");
    assert_eq!(
        second - first,
        10_000_000,
        "one second apart, in 100 ns ticks"
    );
    assert_eq!(
        rows.get(&4300).and_then(|row| row.started_at_ticks),
        None,
        "an unreadable stamp proves nothing"
    );
}

/// The same instant read twice is the same number: the identity check at
/// signal time compares against the plan's read, so a stable parse is the
/// premise of every comparison.
#[test]
fn the_same_lstart_parses_to_the_same_instant() {
    let stamp = "Tue Oct 13 23:59:59 2026";
    assert_eq!(parse_lstart_ms(stamp), parse_lstart_ms(stamp));
    assert!(parse_lstart_ms(stamp).is_some());
}

/// `ps -o pgid= -p PID` pads like every numeric column.
#[test]
fn a_padded_group_is_compared_with_the_group_the_session_leads() {
    assert_eq!(
        group_membership(
            "12102
",
            Some(12102)
        ),
        Membership::Member
    );
    assert_eq!(
        group_membership(
            "  843
",
            Some(843)
        ),
        Membership::Member
    );
    assert_eq!(
        group_membership(
            "  843
",
            Some(12102)
        ),
        Membership::Outside
    );
    assert_eq!(
        group_membership(
            "843
", None
        ),
        Membership::Outside
    );
    assert_eq!(
        group_membership("", Some(12102)),
        Membership::Outside,
        "no longer listed"
    );
    assert_eq!(
        group_membership(
            "not a number
",
            Some(12102)
        ),
        Membership::Unreadable
    );
}
