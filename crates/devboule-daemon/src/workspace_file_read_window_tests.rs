//! The window's own cases over real repositories: the first window of a
//! file too big for one, the continuation at a window edge, the last one,
//! the one past the end, and the line the cap has to cut — each pinned to
//! the numbers the panel builds its next request from (`from_line` +
//! `lines`), so a window that loses or repeats a line dies here. The
//! refusals and the byte classification are in [`super::tests`].

use devboule_protocol::{WorkspaceFileContentKind, WorkspaceFileContentStatus};

use super::fixture::{mtime_ms, Repo};
use super::{content_of, FILE_READ_MAX_BYTES};

/// Mutant `m:window-cap` — let the read run past the cap: this file comes
/// back whole, the size assertion below is what can no longer prove the
/// window exists (and the frame test in `super::tests` loses its meaning
/// with it). The cap lands on a line boundary here — 128 lines of exactly
/// 1 KiB — so the expected window is arithmetic, not a guess.
#[test]
fn the_first_window_of_a_big_file_is_the_cap_and_says_more_follows() {
    let repo = Repo::new("window-big");
    let line = format!("{}\n", "a".repeat(1023)); // 1024 bytes
    repo.write("big.log", line.repeat(400)); // 409_600 bytes, over the cap

    let reply = content_of(&repo.root, "big.log", None, None);

    assert_eq!(reply.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(reply.kind, Some(WorkspaceFileContentKind::Text));
    let content = reply.content.as_deref().expect("the window travels");
    assert_eq!(
        content.len(),
        FILE_READ_MAX_BYTES as usize,
        "128 lines × 1 KiB is exactly the cap"
    );
    assert_eq!(content.lines().count(), 128, "whole lines only");
    assert_eq!(reply.from_line, Some(1));
    assert_eq!(reply.lines, Some(128));
    assert_eq!(
        reply.has_more,
        Some(true),
        "the file is bigger than this window"
    );
    assert_eq!(reply.truncated, Some(false), "the cap landed on a newline");
    assert_eq!(
        reply.size,
        Some(409_600),
        "the file's own size, not the window's"
    );
    assert_eq!(
        reply.modified_at,
        Some(mtime_ms(&repo.root.join("big.log")))
    );
}

/// Mutant `m:offset` — ignore `from_line` and start at the file's first
/// line again: the second window comes back identical to the first, and
/// this case — which derives the next line the way the panel does, from
/// `from_line + lines` — dies on the exact content.
#[test]
fn the_second_window_picks_up_exactly_where_the_first_ended() {
    let repo = Repo::new("window-next");
    let mut file = String::new();
    for number in 1..=10 {
        file.push_str(&format!("line {number}\n"));
    }
    repo.write("lines.txt", file);

    let first = content_of(&repo.root, "lines.txt", None, Some(3));
    assert_eq!(first.content.as_deref(), Some("line 1\nline 2\nline 3\n"));
    assert_eq!(first.from_line, Some(1));
    assert_eq!(first.lines, Some(3));
    assert_eq!(first.has_more, Some(true));

    // The panel's own arithmetic: the next window starts at the line after
    // the last one this reply said it holds.
    let next = first.from_line.expect("a window names its line")
        + first.lines.expect("a window names its lines");
    assert_eq!(next, 4);

    let second = content_of(&repo.root, "lines.txt", Some(next), Some(3));
    assert_eq!(
        second.content.as_deref(),
        Some("line 4\nline 5\nline 6\n"),
        "no line of the first window repeated, no line of this one lost"
    );
    assert_eq!(second.from_line, Some(4));
    assert_eq!(second.lines, Some(3));
    assert_eq!(second.has_more, Some(true));
}

/// Mutant `m:has-more` — answer `true` unconditionally: this last window
/// is the one that can no longer prove the flag means anything, and the
/// panel would offer a «Read more» into nothing.
#[test]
fn the_last_window_says_there_is_no_more() {
    let repo = Repo::new("window-last");
    let mut file = String::new();
    for number in 1..=5 {
        file.push_str(&format!("line {number}\n"));
    }
    repo.write("five.txt", file);

    let last = content_of(&repo.root, "five.txt", Some(4), Some(3));

    assert_eq!(last.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(last.content.as_deref(), Some("line 4\nline 5\n"));
    assert_eq!(last.from_line, Some(4));
    assert_eq!(last.lines, Some(2));
    assert_eq!(
        last.has_more,
        Some(false),
        "the file ends inside this window"
    );
    assert_eq!(last.truncated, Some(false));
}

/// Past the end is a window, not a failure: no lines, nothing to follow
/// them, and not one word of error — the panel stops offering windows.
#[test]
fn a_window_past_the_end_answers_with_no_lines_and_no_error() {
    let repo = Repo::new("window-past-end");
    repo.write("three.txt", "one\ntwo\nthree\n");

    let reply = content_of(&repo.root, "three.txt", Some(999), Some(10));

    assert_eq!(reply.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(reply.kind, Some(WorkspaceFileContentKind::Text));
    assert_eq!(reply.content.as_deref(), Some(""));
    assert_eq!(reply.from_line, Some(999), "the line it was addressed at");
    assert_eq!(reply.lines, Some(0), "zero lines came back");
    assert_eq!(reply.has_more, Some(false));
    assert_eq!(reply.truncated, Some(false));
    assert_eq!(reply.error, None, "past the end is no error");
    assert_eq!(reply.size, Some(14), "the file's own size, unaffected");
}

/// A file with no newline inside the window — one line bigger than the
/// cap — comes back cut at the cap and **says so**: `truncated` is the
/// caller's declaration, `note` its sentence — and `has_more` is **false**,
/// because a continuation this reply cannot deliver byte-exactly is a
/// promise «Read more» must not make: what follows that line is declared
/// unreachable this way, never skipped in silence. Also a size case of
/// mutant `m:window-cap`.
#[test]
fn a_line_bigger_than_the_window_is_cut_and_says_so() {
    let repo = Repo::new("window-giant-line");
    repo.write("one.log", "y".repeat(FILE_READ_MAX_BYTES as usize + 500));

    let reply = content_of(&repo.root, "one.log", None, None);

    assert_eq!(reply.status, WorkspaceFileContentStatus::Ok);
    let content = reply.content.as_deref().expect("the window travels");
    assert_eq!(content.len(), FILE_READ_MAX_BYTES as usize, "the cap, cut");
    assert!(
        !content.ends_with('\n'),
        "a file with no newline ends mid-line, and the window says nothing else"
    );
    assert_eq!(reply.from_line, Some(1));
    assert_eq!(reply.lines, Some(1));
    assert_eq!(reply.truncated, Some(true), "the cut is declared");
    assert_eq!(
        reply.has_more,
        Some(false),
        "no continuation is offered for content that cannot resume byte-exactly"
    );
    assert_eq!(
        reply
            .note
            .as_deref()
            .map(|note| note.contains("exceeds one window")),
        Some(true),
        "the cut travels with its sentence: {:?}",
        reply.note
    );
}

/// The cap lands between two bytes of a three-byte character: without the
/// step back to the last whole character the window would end in a
/// half-character that is not text — or worse, be called binary for a cut
/// this module made itself.
#[test]
fn the_cap_never_cuts_a_character_in_half() {
    let repo = Repo::new("window-utf8");
    // 131_100 bytes: past the 131_072-byte cap, and 131_072 is not a
    // multiple of three, so the raw cut would split a '€'.
    repo.write(
        "euro.log",
        "€".repeat(FILE_READ_MAX_BYTES as usize / 3 + 10),
    );

    let reply = content_of(&repo.root, "euro.log", None, None);

    assert_eq!(
        reply.status,
        WorkspaceFileContentStatus::Ok,
        "a cut this module made is not a reason to call a file binary"
    );
    let content = reply.content.as_deref().expect("the window travels");
    assert_eq!(content.len() % 3, 0, "the window ends on a character edge");
    assert!(
        content.chars().all(|letter| letter == '€'),
        "whole characters only, no half of one"
    );
    assert!(content.len() < FILE_READ_MAX_BYTES as usize, "stepped back");
    assert_eq!(reply.truncated, Some(true), "and the cut is still declared");
    assert_eq!(
        reply.has_more,
        Some(false),
        "the same rule as any line bigger than one window: stop, declared"
    );
}

/// The acceptance walk, driven the way the panel drives it: a first
/// window with no window named (the default request), every later one at
/// `from_line + lines` with the panel's own line budget — and these lines
/// deliberately straddle the cap, so every seam is a line the previous
/// window had to end BEFORE rather than cut through. The accumulated text
/// equal to the file byte for byte is the proof that no seam skips or
/// repeats a byte; the newline invariant inside the loop is the contract
/// the panel's arithmetic stands on. Kills `m:offset` (the second window
/// would repeat the first) and the mutant that ends windows mid-line (the
/// first window comes back cut and stops the walk at its first 128 KiB).
#[test]
fn a_300_kib_file_walks_window_by_window_to_the_end() {
    let repo = Repo::new("window-walk");
    let mut file = String::new();
    for number in 0..3400 {
        file.push_str(&format!("record {number:05} {}\n", "p".repeat(80)));
    }
    assert!(file.len() > 300 * 1024, "the acceptance file is 300 KiB up");
    assert_ne!(
        FILE_READ_MAX_BYTES % 94,
        0,
        "94-byte lines against a 131072-byte cap: these lines must straddle it"
    );
    repo.write("walk.log", file.clone());

    let mut shown = String::new();
    let mut start = 1u64;
    let mut windows = 0u32;
    loop {
        // The panel's first request carries no window fields at all; the
        // later ones carry its arithmetic and its line budget (5000).
        let (from_line, line_count) = if windows == 0 {
            (None, None)
        } else {
            (Some(start), Some(5000))
        };
        let reply = content_of(&repo.root, "walk.log", from_line, line_count);
        assert_eq!(
            reply.status,
            WorkspaceFileContentStatus::Ok,
            "window {start}"
        );
        assert_eq!(reply.from_line, Some(start));
        let content = reply.content.as_deref().expect("the window travels");
        assert!(
            content.len() <= FILE_READ_MAX_BYTES as usize,
            "window {start} under the cap"
        );
        if reply.has_more == Some(true) {
            assert!(
                content.ends_with('\n'),
                "a window that promises more ends on a line — that is what keeps \
                 `from_line + lines` byte-exact: window {start}"
            );
        }
        shown.push_str(content);
        windows += 1;
        assert!(windows < 50, "the walk must end, and it has not after 50");
        if reply.has_more == Some(false) {
            break;
        }
        start += reply.lines.expect("a window names its lines");
    }

    assert!(windows > 1, "300 KiB does not fit one window");
    assert_eq!(shown, file, "every line once — none lost, none repeated");
}

/// The protocol's own tolerance, pinned: `line_count: 0` still hands back
/// one line. The request's doc states it ("Asked with 0 it still takes one
/// line"), so this test exists to keep that choice from drifting into a
/// clamp nobody declared or a refusal nobody asked for.
#[test]
fn a_line_count_of_zero_still_hands_back_one_line() {
    let repo = Repo::new("count-zero");
    repo.write("few.txt", "a\nb\nc\n");

    let reply = content_of(&repo.root, "few.txt", None, Some(0));

    assert_eq!(reply.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(reply.content.as_deref(), Some("a\n"));
    assert_eq!(reply.lines, Some(1));
    assert_eq!(
        reply.has_more,
        Some(true),
        "two lines follow the one that was asked for"
    );
    assert_eq!(reply.truncated, Some(false));
}

/// The cap's byte boundary on all three sides: exactly the cap with a
/// newline on its last byte (the whole file), exactly the cap without one
/// (EOF ends that line — shown, not cut), and one byte over with the
/// newline AT index cap (the line is cap + 1 bytes: bigger than one window
/// — cut, stopped, declared). The first slice only ever hit this edge by
/// luck of the arithmetic; these three files pin it deliberately.
#[test]
fn the_window_boundary_is_pinned_at_the_byte() {
    let repo = Repo::new("boundary");
    let cap = FILE_READ_MAX_BYTES as usize;

    repo.write("cap-nl.txt", format!("{}\n", "x".repeat(cap - 1)));
    let at_newline = content_of(&repo.root, "cap-nl.txt", None, None);
    let content = at_newline.content.as_deref().expect("the window travels");
    assert_eq!(content.len(), cap, "the whole cap comes back");
    assert!(content.ends_with('\n'), "…ending on its last byte");
    assert_eq!(
        at_newline.has_more,
        Some(false),
        "the file ended at the cap"
    );
    assert_eq!(at_newline.truncated, Some(false));
    assert_eq!(at_newline.note, None);

    repo.write("cap-nonl.txt", "x".repeat(cap));
    let at_eof = content_of(&repo.root, "cap-nonl.txt", None, None);
    assert_eq!(at_eof.content.as_deref().map(str::len), Some(cap));
    assert_eq!(at_eof.has_more, Some(false));
    assert_eq!(
        at_eof.truncated,
        Some(false),
        "the file ends the line, the cap does not cut it"
    );
    assert_eq!(at_eof.lines, Some(1));
    assert_eq!(at_eof.note, None);

    repo.write("over.txt", format!("{}\n", "x".repeat(cap)));
    let over = content_of(&repo.root, "over.txt", None, None);
    assert_eq!(over.content.as_deref().map(str::len), Some(cap));
    assert_eq!(
        over.has_more,
        Some(false),
        "nothing after a line this big is offered"
    );
    assert_eq!(over.truncated, Some(true));
    assert_eq!(
        over.note
            .as_deref()
            .map(|note| note.contains("exceeds one window")),
        Some(true),
        "the cut arrives with its sentence: {:?}",
        over.note
    );
}

/// The case the review asked for by name: a first window ending on the
/// LAST byte of the cap — newline and all — and a second one starting on
/// the following line, the two together equal to the file: no line lost,
/// none repeated, at the exact byte boundary.
#[test]
fn the_first_window_can_end_on_the_last_cap_byte_and_the_next_starts_the_following_line() {
    let repo = Repo::new("cap-seam");
    let cap = FILE_READ_MAX_BYTES as usize;
    let mut file = format!("{}\n", "x".repeat(cap - 1));
    file.push_str("tail\n");
    repo.write("seam.txt", file.clone());

    let first = content_of(&repo.root, "seam.txt", None, None);
    let first_content = first.content.as_deref().expect("the window travels");
    assert_eq!(
        first_content.len(),
        cap,
        "the window ends on the cap's last byte"
    );
    assert!(first_content.ends_with('\n'), "…which is a newline");
    assert_eq!(first.lines, Some(1));
    assert_eq!(first.has_more, Some(true));
    assert_eq!(first.truncated, Some(false));

    let next = first.from_line.expect("a window names its line")
        + first.lines.expect("a window names its lines");
    let second = content_of(&repo.root, "seam.txt", Some(next), None);
    let second_content = second.content.as_deref().expect("the window travels");
    assert_eq!(
        second_content, "tail\n",
        "the following line, from its first byte"
    );
    assert_eq!(second.has_more, Some(false));
    assert_eq!(
        format!("{first_content}{second_content}"),
        file,
        "the two windows together are the file: nothing lost, nothing repeated"
    );
}
