//! The addresses agent browsing may not reach: the special-purpose ranges,
//! the IPv6 forms that carry an IPv4 address, and the numeric IPv4 spellings a
//! resolver would otherwise normalise. One question, asked once per address.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// The one spelling of a host this module and the policy compare with.
pub(super) fn normalise_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// The address behind a host that is nothing but an address: IPv6 literals,
/// and the decimal/octal/hex IPv4 forms a resolver would otherwise normalise
/// for us.
pub(super) fn literal_address(host: &str) -> Option<IpAddr> {
    let bare = host.trim_matches(['[', ']']);
    if let Ok(address) = bare.parse::<IpAddr>() {
        return Some(address);
    }
    legacy_ipv4(bare).map(IpAddr::V4)
}

/// Whether a host is a number wearing address clothes that this parser cannot
/// read. Unreadable is not public: it is refused.
pub(super) fn looks_numeric(host: &str) -> bool {
    let mut characters = host.chars();
    let starts_with_digit = characters
        .next()
        .is_some_and(|first| first.is_ascii_digit());
    starts_with_digit
        && host
            .chars()
            .all(|character| character.is_ascii_hexdigit() || matches!(character, '.' | 'x' | 'X'))
}

/// `inet_aton`'s forms: 1 to 4 parts, each decimal, octal (`0…`) or hex
/// (`0x…`), where the first parts are bytes and the last carries the rest.
fn legacy_ipv4(host: &str) -> Option<Ipv4Addr> {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    let numbers: Vec<u64> = parts
        .iter()
        .map(|part| parse_number(part))
        .collect::<Option<_>>()?;
    let mut value: u64 = 0;
    for (index, number) in numbers.iter().enumerate() {
        let width = if index + 1 == numbers.len() {
            4 - index
        } else {
            1
        };
        let limit = (1u64 << (8 * width)) - 1;
        if *number > limit {
            return None;
        }
        value = (value << (8 * width)) | *number;
    }
    Some(Ipv4Addr::from(value as u32))
}

fn parse_number(part: &str) -> Option<u64> {
    if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok();
    }
    if part.len() > 1 && part.starts_with('0') {
        return u64::from_str_radix(&part[1..], 8).ok();
    }
    part.parse::<u64>().ok()
}

pub(super) fn is_localhost(host: &str) -> bool {
    let host = normalise_host(host);
    host == "localhost" || host.ends_with(".localhost")
}

/// Why an address is not reachable for agent browsing, in the words the
/// refusal sentence uses, or `None` when it is a public address.
pub(super) fn address_blocked(address: IpAddr) -> Option<&'static str> {
    match address {
        IpAddr::V4(address) => ipv4_blocked(address),
        IpAddr::V6(address) => ipv6_blocked(address),
    }
}

fn ipv4_blocked(address: Ipv4Addr) -> Option<&'static str> {
    let bytes = address.octets();
    if address.is_loopback() {
        return Some("loopback address");
    }
    if address.is_unspecified() {
        return Some("unspecified address");
    }
    if bytes[0] == 0 {
        return Some("'this network' address");
    }
    if address.is_private() {
        return Some("private address");
    }
    if address.is_link_local() {
        return Some("link-local address");
    }
    if bytes[0] == 100 && (64..128).contains(&bytes[1]) {
        return Some("tailnet (100.64/10) address");
    }
    if bytes[0] == 192 && bytes[1] == 0 && bytes[2] == 0 {
        return Some("IETF protocol assignment address");
    }
    if bytes[0] == 198 && (bytes[1] == 18 || bytes[1] == 19) {
        return Some("benchmarking address");
    }
    if address.is_multicast() {
        return Some("multicast address");
    }
    if address == Ipv4Addr::BROADCAST {
        return Some("broadcast address");
    }
    if bytes[0] >= 240 {
        return Some("reserved address");
    }
    None
}

fn ipv6_blocked(address: Ipv6Addr) -> Option<&'static str> {
    if address.is_loopback() {
        return Some("loopback address");
    }
    if address.is_unspecified() {
        return Some("unspecified address");
    }
    if address.is_unicast_link_local() {
        return Some("link-local address");
    }
    if address.is_unique_local() {
        return Some("unique-local address");
    }
    if address.is_multicast() {
        return Some("multicast address");
    }
    let segments = address.segments();
    if segments[0] == 0x64 && segments[1] == 0xff9b {
        return Some("NAT64 address");
    }
    if segments[0] == 0x100 && segments[1] == 0 && segments[2] == 0 && segments[3] == 0 {
        return Some("discard-only address");
    }
    if segments[0] == 0x2001 && segments[1] == 0xdb8 {
        return Some("documentation address");
    }
    // An IPv4 address wearing an IPv6 spelling: decide by what it carries.
    if let Some(mapped) = address.to_ipv4_mapped() {
        return ipv4_blocked(mapped).map(|_| "IPv4-mapped address");
    }
    if let Some(compatible) = compatible_ipv4(address) {
        return ipv4_blocked(compatible).map(|_| "IPv4-compatible address");
    }
    None
}

/// `::a.b.c.d`, the form RFC 4291 retired and resolvers still hand out. `::`
/// and `::1` are their own cases already handled above.
fn compatible_ipv4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    if segments[0..6] != [0, 0, 0, 0, 0, 0] || (segments[6] == 0 && segments[7] <= 1) {
        return None;
    }
    Some(Ipv4Addr::new(
        (segments[6] >> 8) as u8,
        segments[6] as u8,
        (segments[7] >> 8) as u8,
        segments[7] as u8,
    ))
}
