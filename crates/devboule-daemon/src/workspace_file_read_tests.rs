//! `content_of` over real repositories in `%TEMP%`: the answers that hand
//! bytes back, the image's withholding, and the refusals — each pinned to
//! the sentence the rest of the panel already shows, and none of the
//! refusals carrying the workspace root. Every case here is the **default**
//! request — no window named, which the daemon reads as the first one —
//! plus one that names a window, to pin that no confinement answer moves
//! when it does. The classification rules themselves (NUL, UTF-8, the
//! extension list) live only here, driven through real files; the window's
//! own cases — where the cap cuts, what `has_more` says, the line past the
//! end — are in [`super::window_tests`].

use std::path::Path;

use base64::Engine;
use devboule_protocol::{
    DaemonMessage, WorkspaceFileContentKind, WorkspaceFileContentStatus, MAX_FRAME_BYTES,
};

use super::fixture::{mtime_ms, unique_directory, Repo};
use super::{content_of, FILE_READ_MAX_BYTES};

#[test]
fn a_text_file_comes_back_with_its_bytes_size_and_mtime() {
    let repo = Repo::new("text");
    repo.write("docs/hello.txt", "hello\n");

    let reply = content_of(&repo.root, "docs/hello.txt", None, None);

    assert_eq!(reply.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(reply.kind, Some(WorkspaceFileContentKind::Text));
    assert_eq!(reply.content.as_deref(), Some("hello\n"));
    assert_eq!(reply.size, Some(6));
    assert_eq!(
        reply.modified_at,
        Some(mtime_ms(&repo.root.join("docs/hello.txt"))),
        "the file's own stamp, in milliseconds since the epoch"
    );
    assert_eq!(reply.error, None);
    // The default request's own window, stated rather than implied: one
    // window, starting at the first line, ending at the file.
    assert_eq!(reply.from_line, Some(1));
    assert_eq!(reply.lines, Some(1));
    assert_eq!(reply.has_more, Some(false));
    assert_eq!(reply.truncated, Some(false));
}

/// Mutant `m:cap` — drop the stat's cap check: the over-cap image comes
/// back `ok` with base64 the frame cannot carry, and this case is the one
/// that can no longer prove the cap exists. An image is the road with no
/// window (base64 cut in the middle decodes to nothing), so it alone is
/// withheld whole: the measure travels in the sentence, the real size in
/// `size`, and no content at all.
#[test]
fn the_cap_still_withholds_an_image_whole_and_carries_the_measure_not_the_content() {
    let repo = Repo::new("cap");
    repo.write("at-cap.png", vec![b'a'; FILE_READ_MAX_BYTES as usize]);
    let at_cap = content_of(&repo.root, "at-cap.png", None, None);
    assert_eq!(
        at_cap.status,
        WorkspaceFileContentStatus::Ok,
        "the cap is inclusive: exactly at it, the content comes back"
    );
    let expected =
        base64::engine::general_purpose::STANDARD.encode(vec![b'a'; FILE_READ_MAX_BYTES as usize]);
    assert_eq!(at_cap.content.as_deref(), Some(expected.as_str()));
    assert_eq!(
        at_cap.from_line, None,
        "an image has no window: base64 is not lines"
    );

    repo.write("over.png", vec![b'a'; FILE_READ_MAX_BYTES as usize + 1]);
    let reply = content_of(&repo.root, "over.png", None, None);
    assert_eq!(reply.status, WorkspaceFileContentStatus::TooLarge);
    assert_eq!(reply.content, None, "a withheld file hands back no bytes");
    assert_eq!(
        reply.kind, None,
        "the bytes were never read, so their class is unknown"
    );
    assert_eq!(reply.size, Some(FILE_READ_MAX_BYTES + 1));
    assert_eq!(
        reply.modified_at,
        Some(mtime_ms(&repo.root.join("over.png")))
    );
    let error = reply.error.as_deref().expect("the measure travels");
    assert!(
        error.contains(&FILE_READ_MAX_BYTES.to_string()),
        "the cap spelled out: {error}"
    );
}

/// The listing's own spelling-blind guard, run before any stat of the
/// metadata folder: every spelling Win32 resolves to the real `.git`
/// refuses with the sentence the tree already shows.
#[test]
fn the_git_folder_is_refused_when_named_directly() {
    let repo = Repo::new("git-direct");
    assert!(
        repo.root.join(".git/config").is_file(),
        "the fixture's .git must be real for this case to mean anything"
    );

    for requested in [
        ".git",
        ".GIT",
        ".git.",
        ".git ",
        ".git/config",
        ".GIT/config",
    ] {
        let reply = content_of(&repo.root, requested, None, None);
        assert_eq!(
            reply.status,
            WorkspaceFileContentStatus::Refused,
            "{requested}"
        );
        let error = reply.error.as_deref().expect("the refusal says why");
        assert!(
            error.contains("not part of the tree"),
            "{requested}: {error}"
        );
        assert_eq!(reply.content, None, "{requested}");
        assert_eq!(reply.size, None, "a refusal claims nothing: {requested}");
    }
}

#[test]
fn paths_that_leave_the_workspace_are_refused() {
    let repo = Repo::new("escape");
    repo.write("in.txt", "a\n");

    for requested in ["../escape.txt", "/absolute/escape.txt"] {
        let reply = content_of(&repo.root, requested, None, None);
        assert_eq!(
            reply.status,
            WorkspaceFileContentStatus::Refused,
            "{requested}"
        );
        let error = reply.error.as_deref().expect("the refusal says why");
        assert!(
            error.contains("outside the workspace folder"),
            "{requested}: {error}"
        );
        assert_eq!(reply.content, None, "{requested}");
    }
}

/// Mutant `m:walk` — skip the walk and stat the target alone: the junction
/// resolves, its target's bytes come back, and this case dies. Both link
/// kinds get the SAME sentences the rest of the panel shows (shared in
/// `workspace_git_support`, never copied) — under the default request and
/// under a named window alike, because the window is addressed only after
/// the walk has said the path is inside.
#[test]
#[cfg(windows)]
fn a_path_through_or_to_a_link_is_refused_with_the_walk_sentences() {
    let repo = Repo::new("links");
    repo.write("in.txt", "a\n");
    let outside = unique_directory("links-target");
    std::fs::write(outside.join("present.txt"), "outside reached\n").expect("outside file");
    let junction = repo.root.join("dirlink");
    let created = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .expect("mklink");
    assert!(
        created.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let dangling = repo.root.join("vanished.txt");
    std::os::windows::fs::symlink_file(outside.join("present.txt"), &dangling).expect("symlink");

    for (requested, fragment) in [
        ("dirlink", "symbolic link"),
        ("dirlink/present.txt", "crosses a link"),
        ("vanished.txt", "symbolic link"),
    ] {
        for (from_line, line_count) in [(None, None), (Some(7), Some(2))] {
            let reply = content_of(&repo.root, requested, from_line, line_count);
            assert_eq!(
                reply.status,
                WorkspaceFileContentStatus::Refused,
                "{requested} (window {from_line:?}+{line_count:?})"
            );
            assert_eq!(reply.content, None, "{requested}");
            let error = reply.error.as_deref().expect("the refusal says why");
            assert!(
                error.contains(fragment),
                "{requested} (window {from_line:?}+{line_count:?}): {error}"
            );
            // The same assertion the no-root list runs: a link refusal that
            // starts naming the path it refused dies here too.
            assert_no_root(error, &repo.root);
            assert!(
                !format!("{reply:?}").contains("outside reached"),
                "{requested} read through the link: {reply:?}"
            );
        }
    }
    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir(&junction);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn a_missing_file_is_refused_with_the_listing_own_sentence() {
    let repo = Repo::new("missing");
    repo.write("in.txt", "a\n");

    let reply = content_of(&repo.root, "never-existed.txt", None, None);

    assert_eq!(reply.status, WorkspaceFileContentStatus::Refused);
    let error = reply.error.as_deref().expect("the refusal says why");
    assert!(error.contains("does not exist"), "{error}");
    assert_eq!(reply.content, None);
    assert_eq!(reply.size, None, "a refusal claims nothing");
}

/// Mutant `m:zero-line` — let the zero resolve to the first window, which
/// is what the review caught: this case dies on the refusal it must get
/// instead. Lines are numbered from 1; a zero is the caller's mistake and
/// gets this frame's own sentence, never a window it did not ask for.
#[test]
fn a_from_line_of_zero_is_refused_with_its_own_sentence() {
    let repo = Repo::new("zero-line");
    repo.write("in.txt", "a\nb\n");

    let reply = content_of(&repo.root, "in.txt", Some(0), Some(10));

    assert_eq!(reply.status, WorkspaceFileContentStatus::Refused);
    let error = reply.error.as_deref().expect("the refusal says why");
    assert!(error.contains("lines are numbered from 1"), "{error}");
    assert_eq!(reply.content, None, "line 0 hands back nothing");
    assert_eq!(reply.size, None, "a refusal claims nothing");
    assert_eq!(reply.from_line, None, "a refusal carries no window");
    assert_eq!(reply.lines, None);
    assert_eq!(reply.has_more, None);
    assert_eq!(reply.truncated, None);
    assert_eq!(reply.note, None);
    assert_no_root(error, &repo.root);
}

/// A named window asks for the same refusals, word for word — confinement
/// runs before any window is located, so no line number can widen it. The
/// link case runs its own second pass above (it needs Win32 to exist).
#[test]
fn a_named_window_gets_the_same_refusals_as_the_default_request() {
    let repo = Repo::new("window-refusals");
    repo.write("in.txt", "a\n");
    repo.write("src/lib.rs", "//\n");

    for (requested, fragment) in [
        ("../escape.txt", "outside the workspace folder"),
        ("/absolute/escape.txt", "outside the workspace folder"),
        (".git/config", "not part of the tree"),
        ("never-existed.txt", "does not exist"),
        ("src", "is a folder, not a file"),
    ] {
        let reply = content_of(&repo.root, requested, Some(5), Some(10));
        assert_eq!(
            reply.status,
            WorkspaceFileContentStatus::Refused,
            "{requested}"
        );
        let error = reply.error.as_deref().expect("the refusal says why");
        assert!(error.contains(fragment), "{requested}: {error}");
        assert_eq!(reply.content, None, "{requested}");
        assert_eq!(reply.size, None, "a refusal claims nothing: {requested}");
        assert_eq!(
            reply.from_line, None,
            "a refusal carries no window: {requested}"
        );
        assert_no_root(error, &repo.root);
    }
}

/// Mutant `m:binary` — hand content back for a binary anyway: the reply
/// must refuse the bytes, not pass a NUL run off as text.
#[test]
fn a_binary_file_comes_back_with_its_stat_and_no_content() {
    let repo = Repo::new("binary");
    let bytes = [0x00u8, 0x01, 0xFF, 0xFE, 0x41];
    repo.write("blob.dat", bytes);

    let reply = content_of(&repo.root, "blob.dat", None, None);

    assert_eq!(reply.status, WorkspaceFileContentStatus::Binary);
    assert_eq!(reply.kind, Some(WorkspaceFileContentKind::Binary));
    assert_eq!(
        reply.content, None,
        "binary means no content, never a lossy decode"
    );
    assert_eq!(reply.size, Some(5));
    assert_eq!(
        reply.modified_at,
        Some(mtime_ms(&repo.root.join("blob.dat")))
    );
    assert_eq!(reply.error, None, "binary is an answer, not a failure");
    assert_eq!(
        reply.from_line, None,
        "no window over bytes nobody can page through"
    );
    assert_eq!(reply.lines, None);
    assert_eq!(reply.has_more, None);
    assert_eq!(reply.truncated, None);
}

/// The reason the extension list exists: an image's bytes are binary, and
/// without recognizing the spelling they would never be shown at all.
#[test]
fn an_image_is_recognized_by_its_extension_and_carries_base64() {
    let repo = Repo::new("image");
    let bytes = [0x89u8, b'P', b'N', b'G', 0x00, 0xFF, 0x0D, 0x0A];
    repo.write("shot.png", bytes);

    let reply = content_of(&repo.root, "shot.png", None, None);

    assert_eq!(reply.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(reply.kind, Some(WorkspaceFileContentKind::Image));
    let expected = base64::engine::general_purpose::STANDARD.encode(bytes);
    assert_eq!(reply.content.as_deref(), Some(expected.as_str()));
    assert_eq!(reply.size, Some(8));
}

/// The two things a refusal sentence must never carry: this run's workspace
/// root, and any path-shaped text at all (`/`, `\`, `:` — which catches a
/// leaked *relative* path too). One assertion, two callers: the no-root list
/// below and the link and window tests — a `format!` added at either call
/// site dies.
fn assert_no_root(error: &str, root: &Path) {
    let root = root.to_string_lossy().into_owned();
    assert!(!error.contains(root.as_str()), "the root leaked: {error}");
    assert!(
        !error.contains('/') && !error.contains('\\') && !error.contains(':'),
        "a path leaked: {error}"
    );
}

/// Mutant `m:sentence` — build one of these sentences from the path instead
/// of a constant: the root leaks into `error` and this assertion dies.
/// The list holds every refusal class `content_of` composes around the
/// confined path — including the frame's own zero-line refusal, whose
/// sentence must be as path-free as the rest; the two link refusals run
/// [`assert_no_root`] in their own test above, so all seven are covered and
/// no arm is skipped. `READ_FAILED`
/// is in neither list: a bare constant with no interpolation, on an arm no
/// test can force open (declared in the report).
#[test]
fn no_refusal_sentence_carries_the_workspace_root() {
    let repo = Repo::new("pathless");
    repo.write("src/lib.rs", "//\n");

    let refusals = [
        content_of(&repo.root, "", None, None),
        content_of(&repo.root, ".", None, None),
        content_of(&repo.root, "../escape.txt", None, None),
        content_of(&repo.root, "/absolute/escape.txt", None, None),
        content_of(&repo.root, ".git/config", None, None),
        content_of(&repo.root, "never-existed.txt", None, None),
        content_of(&repo.root, "src", None, None),
        // …and the frame's own refusal, on a path that would otherwise be
        // read: its sentence must carry no more of the path than the others.
        content_of(&repo.root, "src/lib.rs", Some(0), None),
    ];
    for reply in refusals {
        assert_eq!(
            reply.status,
            WorkspaceFileContentStatus::Refused,
            "{reply:?}"
        );
        assert_no_root(
            reply.error.as_deref().expect("a refusal says why"),
            &repo.root,
        );
    }
}

/// The worst content this reply can carry — a file at the cap where every
/// byte is a control character, each escaping to six in JSON — serialized
/// the way the wire serializes it: one line, under `MAX_FRAME_BYTES`.
/// Together with the window test that pins the cap itself, this is what
/// keeps 128 KiB derived physics rather than a preference.
#[test]
fn the_worst_content_still_fits_one_frame() {
    let repo = Repo::new("frame");
    repo.write("controls.txt", vec![0x01u8; FILE_READ_MAX_BYTES as usize]);

    let reply = content_of(&repo.root, "controls.txt", None, None);
    assert_eq!(
        reply.status,
        WorkspaceFileContentStatus::Ok,
        "0x01 is valid UTF-8 with no NUL: this is text"
    );

    let frame = DaemonMessage::WorkspaceFileContent { id: 1, file: reply };
    let json = serde_json::to_string(&frame).expect("serialize");
    assert!(
        json.len() < MAX_FRAME_BYTES,
        "{} bytes is not under the {}-byte frame cap",
        json.len(),
        MAX_FRAME_BYTES
    );
}
