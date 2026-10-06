//! The listener-table parsers on hand-built buffers: each family's own row
//! layout, and the claimed-count clamp that keeps a lying table header from
//! driving a read past the bytes the kernel returned.

use super::*;

fn v4_row(port: u16, pid: u32) -> Vec<u8> {
    let row = MIB_TCPROW_OWNER_PID {
        dwState: 2,
        dwLocalAddr: 0,
        dwLocalPort: u16::to_be(port) as u32,
        dwRemoteAddr: 0,
        dwRemotePort: 0,
        dwOwningPid: pid,
    };
    let mut bytes = vec![0u8; mem::size_of::<MIB_TCPROW_OWNER_PID>()];
    // SAFETY: the destination is exactly `row`'s size.
    unsafe {
        ptr::copy_nonoverlapping(
            &row as *const MIB_TCPROW_OWNER_PID as *const u8,
            bytes.as_mut_ptr(),
            mem::size_of::<MIB_TCPROW_OWNER_PID>(),
        );
    }
    bytes
}

fn v6_row(port: u16, pid: u32) -> Vec<u8> {
    let row = MIB_TCP6ROW_OWNER_PID {
        dwLocalPort: u16::to_be(port) as u32,
        dwState: 2,
        dwOwningPid: pid,
        ..Default::default()
    };
    let mut bytes = vec![0u8; mem::size_of::<MIB_TCP6ROW_OWNER_PID>()];
    // SAFETY: the destination is exactly `row`'s size.
    unsafe {
        ptr::copy_nonoverlapping(
            &row as *const MIB_TCP6ROW_OWNER_PID as *const u8,
            bytes.as_mut_ptr(),
            mem::size_of::<MIB_TCP6ROW_OWNER_PID>(),
        );
    }
    bytes
}

fn table(claimed: u32, rows: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = claimed.to_le_bytes().to_vec();
    for row in rows {
        bytes.extend_from_slice(row);
    }
    bytes
}

#[test]
fn the_ipv4_table_parses_its_own_row_layout() {
    let bytes = table(2, &[v4_row(1420, 100), v4_row(8080, 4242)]);
    let parsed = parse_owner_pid_table(&bytes, u32::from(AF_INET));
    assert_eq!(parsed, vec![(1420, 100), (8080, 4242)]);
    // A row owned by pid 0 is the table's own padding, never an owner.
    let bytes = table(1, &[v4_row(9, 0)]);
    assert!(parse_owner_pid_table(&bytes, u32::from(AF_INET)).is_empty());
}

#[test]
fn the_ipv6_table_parses_with_the_v6_row_size() {
    // The v6 row is 56 bytes where the v4 row is 24: walking it with the
    // v4 stride would read past the buffer and attribute random owners.
    assert_eq!(mem::size_of::<MIB_TCP6ROW_OWNER_PID>(), 56);
    assert_eq!(mem::size_of::<MIB_TCPROW_OWNER_PID>(), 24);
    let bytes = table(2, &[v6_row(8443, 777), v6_row(9999, 778)]);
    let parsed = parse_owner_pid_table(&bytes, u32::from(AF_INET6));
    assert_eq!(parsed, vec![(8443, 777), (9999, 778)]);
}

#[test]
fn a_claimed_row_count_beyond_the_bytes_returns_clamps_instead_of_reading() {
    let full = table(2, &[v4_row(1, 10), v4_row(2, 11)]);
    let lying = table(4_000, &[v4_row(1, 10), v4_row(2, 11)]);
    assert_eq!(
        parse_owner_pid_table(&full, u32::from(AF_INET)),
        parse_owner_pid_table(&lying, u32::from(AF_INET)),
        "the clamp makes the overclaim equal the real table"
    );
    // A claim with a truncated final row: only whole rows are walked.
    let mut truncated = table(2, &[v4_row(1, 10)]);
    truncated.extend_from_slice(&[0u8; 8]);
    assert_eq!(
        parse_owner_pid_table(&truncated, u32::from(AF_INET)),
        vec![(1, 10)]
    );
    let lying_v6 = table(99, &[v6_row(443, 42)]);
    assert_eq!(
        parse_owner_pid_table(&lying_v6, u32::from(AF_INET6)),
        vec![(443, 42)]
    );
    // Anything shorter than the header is an empty table, not a panic.
    assert!(parse_owner_pid_table(&[1, 2], u32::from(AF_INET)).is_empty());
}
