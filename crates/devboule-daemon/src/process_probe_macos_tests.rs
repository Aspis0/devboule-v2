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
