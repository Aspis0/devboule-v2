//! The route's pieces that need no socket: the request parser (clamp and
//! validation), the unreadable-store phrase, and the source pins that keep
//! the seams no refusal branch can exercise directly.

use std::path::PathBuf;
use std::thread;

use oracle_core::OracleDataPaths;

use crate::oracle::endpoint_query::{no_vectors_message, parse_request, WarmSlot};
use crate::oracle::folder::{Artifact, FolderIndexProbe, ManifestProbe};

/// Clamp and validation, one parser call per case.
#[test]
fn the_parser_clamps_the_limit_and_validates_the_query() {
    let request =
        parse_request(br#"{"root":"C:\\proj","query":"q"}"#).expect("an absent limit is valid");
    assert_eq!(
        request.limit, 10,
        "absent limit defaults to the query limit"
    );
    assert_eq!(
        parse_request(br#"{"root":"C:\\proj","query":"q","limit":5}"#)
            .expect("an in-range limit")
            .limit,
        5
    );
    assert_eq!(
        parse_request(br#"{"root":"C:\\proj","query":"q","limit":0}"#)
            .expect("zero is clamped, not rejected")
            .limit,
        1,
        "limit 0 must clamp up to 1"
    );
    assert_eq!(
        parse_request(br#"{"root":"C:\\proj","query":"q","limit":99}"#)
            .expect("a large limit is clamped, not rejected")
            .limit,
        10,
        "limit 99 must clamp down to the query limit"
    );

    let bogus = parse_request(br#"{"root":"C:\\proj","query":"q","limit":"ten"}"#)
        .expect_err("a string limit is not a number");
    assert!(
        bogus.message.contains("not valid JSON"),
        "{}",
        bogus.message
    );
    // A negative value never clamps: `Option<usize>` fails serde and the
    // route answers 400 — a malformed request is refused, not clamped (F-3).
    let negative = parse_request(br#"{"root":"C:\\proj","query":"q","limit":-5}"#)
        .expect_err("a negative limit is malformed, not clamped");
    assert!(
        negative.message.contains("not valid JSON"),
        "{}",
        negative.message
    );

    let empty = parse_request(br#"{"root":"C:\\proj","query":"   "}"#)
        .expect_err("an empty query is refused");
    assert!(
        empty.message.contains("cannot be empty"),
        "{}",
        empty.message
    );

    let long = format!(r#"{{"root":"C:\\proj","query":"{}"}}"#, "a".repeat(4097));
    let too_long = parse_request(long.as_bytes()).expect_err("a 4097-character query");
    assert!(
        too_long.message.contains("too long"),
        "{}",
        too_long.message
    );

    let no_root = parse_request(br#"{"query":"q"}"#).expect_err("a missing root");
    assert!(
        no_root.message.contains("workspace root"),
        "{}",
        no_root.message
    );
    let relative = parse_request(br#"{"root":"rel/x","query":"q"}"#).expect_err("a relative root");
    assert!(
        relative.message.contains("absolute"),
        "{}",
        relative.message
    );
}

/// The warm slot frees itself however a warm ends: a thread that panics
/// while holding the guard must not leave the slot claimed (every later
/// query would answer `warming` forever), and a guard dropped on the normal
/// path must free it too. A `Drop` that never resets the flag kills this
/// test on both halves.
#[test]
fn the_warm_slot_frees_itself_after_a_panic_and_on_the_normal_path() {
    let slot = WarmSlot::new();
    let guard = slot.try_begin().expect("a fresh slot must be claimable");
    assert!(
        slot.try_begin().is_none(),
        "a held claim must refuse a second warm"
    );

    let panicking = thread::spawn(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _held = guard;
            panic!("the warm load panicked while holding the guard");
        }));
        assert!(
            outcome.is_err(),
            "the panic inside the load must reach catch_unwind"
        );
    });
    panicking
        .join()
        .expect("the thread must survive its own caught panic");
    assert!(
        slot.try_begin().is_some(),
        "a panic while the guard is held must not leave the slot claimed"
    );

    let guard = slot.try_begin().expect("the slot must be claimable again");
    drop(guard);
    assert!(
        slot.try_begin().is_some(),
        "a guard dropped on the normal path must free the slot"
    );
}

/// The other half of the vector refusal: a store that exists but cannot be
/// read. On Windows the probe cannot be forced into this state in I/O —
/// `fs::metadata` still succeeds on an ACL-denied directory — so the phrase
/// itself is pinned here instead (measured, declared in the report).
#[test]
fn the_unreadable_store_refusal_names_the_store() {
    let root = PathBuf::from(r"C:\ws\project");
    let probe = FolderIndexProbe {
        root: root.clone(),
        data: OracleDataPaths::from_root_without_env(&root),
        read_error: None,
        metadata: Artifact::Present,
        chunks: Artifact::Unreadable("access is denied".to_string()),
        manifest: ManifestProbe::Absent,
    };
    let message = no_vectors_message(&probe);
    assert!(
        message.starts_with("Oracle cannot read the chunk vector store")
            && message.contains(probe.data.chunks.display().to_string().as_str())
            && message.contains("access is denied"),
        "the unreadable-store refusal must name the store and the error: {message}"
    );
}

/// Source pins, in the shape of `backend/tests/async_roads.rs`: the seams
/// that no refusal branch can exercise directly — the clamp inside
/// `search_paths`, the two panel callers' limit, and the host awaiting the
/// engine off the window's thread.
#[test]
fn the_query_wiring_is_pinned() {
    let query = include_str!("../query.rs");
    let commands = include_str!("../commands.rs");
    let folder = include_str!("../folder.rs");
    let endpoint_query = include_str!("../endpoint_query.rs");

    assert!(
        query.contains("limit.clamp(1, QUERY_LIMIT)"),
        "search_paths must clamp its limit parameter"
    );
    assert!(
        !query.contains("QUERY_LIMIT.min(MAX_BOUNDED_LIMIT)"),
        "search_paths still hardcodes the limit"
    );
    assert_eq!(
        commands.matches("QUERY_LIMIT").count(),
        2,
        "oracle_ask_inner must import and pass QUERY_LIMIT"
    );
    assert_eq!(
        folder.matches("QUERY_LIMIT").count(),
        2,
        "oracle_ask_folder_inner must import and pass QUERY_LIMIT"
    );
    assert!(
        endpoint_query.contains("block_on(search_paths("),
        "the host must await the engine, not the window's thread"
    );
    assert!(
        endpoint_query.contains("host.search("),
        "respond must reach the search through the host"
    );
}
