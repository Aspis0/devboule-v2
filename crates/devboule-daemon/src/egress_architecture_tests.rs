//! The architecture rule behind the egress policy: no HTTP client and no raw
//! TCP connect to a remote host outside the guarded module. A new client is a
//! red test until someone either routes it through `egress_client` or names it
//! here with the reason it is different.

use std::fs;
use std::path::{Path, PathBuf};

/// What an ad hoc outbound client looks like in source.
const CLIENT_MARKERS: [&str; 5] = [
    "reqwest::",
    "ureq::",
    "hyper::client",
    "TcpStream::connect",
    "UdpSocket::connect",
];

/// Files that may hold a marker, each with why. Every entry must still hold
/// one, so a stale line fails as loudly as a new client.
const DAEMON_ALLOWED: [(&str, &str); 6] = [
    (
        "egress_client.rs",
        "the guarded client itself: the one place reqwest is built",
    ),
    (
        "egress_policy.rs",
        "parses the URL type the client uses; makes no request",
    ),
    (
        "server/peer_dial.rs",
        "the peer transport: a paired device's pinned key and Noise handshake",
    ),
    (
        "pairing.rs",
        "the pairing dial to the address the person typed, under SPAKE2",
    ),
    (
        "tailscale_app_localapi.rs",
        "the Tailscale app's LocalAPI on this machine's own loopback",
    ),
    (
        "bin/",
        "test stub binaries that connect to the broker's loopback endpoint",
    ),
];

/// The one client outside the daemon crate that the daemon links: a model
/// download from fixed constant Hugging Face URLs, nothing an agent names.
const ORACLE_CORE_ALLOWED: [(&str, &str); 1] = [(
    "model_download.rs",
    "fixed https model URLs with its own https-only redirect rule; an async multi-GB stream, not migrated",
)];

fn source_files(dir: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("a source directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            source_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

fn is_test_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with("_tests.rs") || name == "tests.rs")
}

/// Whether a source file holds a client marker on a line that is code.
fn holds_client(source: &str) -> bool {
    source.lines().any(|line| {
        !line.trim_start().starts_with("//")
            && CLIENT_MARKERS.iter().any(|marker| line.contains(marker))
    })
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .expect("under the root")
        .to_string_lossy()
        .replace('\\', "/")
}

fn allowed(relative: &str, allowlist: &[(&str, &str)]) -> bool {
    allowlist.iter().any(|(entry, _)| {
        relative == *entry || (entry.ends_with('/') && relative.starts_with(entry))
    })
}

/// The files under `root` that hold a marker and are not on the allowlist, and
/// the allowlist entries that no longer hold one.
fn audit(root: &Path, allowlist: &[(&str, &str)]) -> (Vec<String>, Vec<String>) {
    let mut files = Vec::new();
    source_files(root, &mut files);
    let holding: Vec<String> = files
        .iter()
        .filter(|path| !is_test_file(path))
        .filter(|path| holds_client(&fs::read_to_string(path).expect("a source file")))
        .map(|path| relative(root, path))
        .collect();
    let stray = holding
        .iter()
        .filter(|name| !allowed(name, allowlist))
        .cloned()
        .collect();
    let stale = allowlist
        .iter()
        .filter(|(entry, _)| !holding.iter().any(|name| allowed(name, &[(*entry, "")])))
        .map(|(entry, _)| (*entry).to_string())
        .collect();
    (stray, stale)
}

#[test]
fn no_ad_hoc_http_client() {
    let daemon = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let (stray, stale) = audit(&daemon, &DAEMON_ALLOWED);
    assert_eq!(
        stray,
        Vec::<String>::new(),
        "an HTTP client or raw connect outside the guarded module: route it through `egress_client`"
    );
    assert_eq!(
        stale,
        Vec::<String>::new(),
        "an allowlist entry that no longer holds a client"
    );
}

#[test]
fn the_linked_oracle_core_client_is_the_one_declared_exception() {
    let oracle = Path::new(env!("CARGO_MANIFEST_DIR")).join("../oracle-core/src");
    let (stray, stale) = audit(&oracle, &ORACLE_CORE_ALLOWED);
    assert_eq!(stray, Vec::<String>::new(), "a new client in oracle-core");
    assert_eq!(stale, Vec::<String>::new(), "the model download moved");
}

#[test]
fn the_scan_sees_a_client_and_ignores_a_comment() {
    assert!(holds_client("let c = reqwest::blocking::Client::new();"));
    assert!(holds_client("let s = std::net::TcpStream::connect(addr)?;"));
    assert!(holds_client("ureq::get(url).call()"));
    assert!(!holds_client("// reqwest::get is not allowed here"));
    assert!(!holds_client("let name = \"reqwest\";"));
}
