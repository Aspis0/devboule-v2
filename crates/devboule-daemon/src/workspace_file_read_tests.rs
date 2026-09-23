//! `content_of` over real repositories in `%TEMP%`: the answers that hand
//! bytes back, the two withholdings, and the refusals — each pinned to the
//! sentence the rest of the panel already shows, and none of the refusals
//! carrying the workspace root. The classification rules themselves (NUL,
//! UTF-8, the extension list) live only here, driven through real files.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use devboule_protocol::{
    DaemonMessage, WorkspaceFileContentKind, WorkspaceFileContentStatus, MAX_FRAME_BYTES,
};

use super::{content_of, FILE_READ_MAX_BYTES};

/// A repository under `temp_dir`, the fixture this slice's brief names: the
/// panel reads files of a real checkout, and the `.git` the guard refuses
/// here is the real metadata folder — `config` included — rather than one a
/// test pretended with. Hard-requires git — a silent skip would let the
/// `.git` cases below pass without running anything.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "devboule-file-read-{label}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir(&root).expect("test directory");
        let repo = Self { root };
        repo.run(&["init", "--quiet"]);
        repo
    }

    fn run(&self, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(arguments)
            .output()
            .expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A unique path under `temp_dir`, for a directory the test wants *outside*
/// the workspace (the link targets of the escape cases).
fn unique_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-file-read-{label}-{}-{stamp}",
        std::process::id()
    ))
}

/// The stat's own mtime, spelled the way the module spells it — the unit
/// (milliseconds) and the epoch are what the wire pins, so the test
/// computes them from the file rather than trusting a hard-coded number.
fn mtime_ms(path: &std::path::Path) -> i64 {
    let time = std::fs::metadata(path)
        .expect("stat")
        .modified()
        .expect("mtime");
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as i64,
        Err(before) => -(before.duration().as_millis() as i64),
    }
}

#[test]
fn a_text_file_comes_back_with_its_bytes_size_and_mtime() {
    let repo = Repo::new("text");
    repo.write("docs/hello.txt", "hello\n");

    let reply = content_of(&repo.root, "docs/hello.txt");

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
}

/// Mutant `m:cap` — drop the stat's cap check: the over-cap file comes
/// back `ok` with 128 KiB + 1 bytes of content, and the frame-size test
/// below is the one that can no longer prove the cap exists. The measure
/// travels in the sentence; the real size travels in `size`.
#[test]
fn the_cap_decides_by_one_byte_and_too_large_carries_the_measure_not_the_content() {
    let repo = Repo::new("cap");
    repo.write("at-cap.txt", "a".repeat(FILE_READ_MAX_BYTES as usize));
    let at_cap = content_of(&repo.root, "at-cap.txt");
    assert_eq!(
        at_cap.status,
        WorkspaceFileContentStatus::Ok,
        "the cap is inclusive: exactly at it, the content comes back"
    );
    assert_eq!(
        at_cap.content.as_deref().map(str::len),
        Some(FILE_READ_MAX_BYTES as usize)
    );

    repo.write("over.txt", "a".repeat(FILE_READ_MAX_BYTES as usize + 1));
    let reply = content_of(&repo.root, "over.txt");
    assert_eq!(reply.status, WorkspaceFileContentStatus::TooLarge);
    assert_eq!(reply.content, None, "a withheld file hands back no bytes");
    assert_eq!(
        reply.kind, None,
        "the bytes were never read, so their class is unknown"
    );
    assert_eq!(reply.size, Some(FILE_READ_MAX_BYTES + 1));
    assert_eq!(
        reply.modified_at,
        Some(mtime_ms(&repo.root.join("over.txt")))
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
        let reply = content_of(&repo.root, requested);
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
        let reply = content_of(&repo.root, requested);
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
/// `workspace_git_support`, never copied).
#[test]
#[cfg(windows)]
fn a_path_through_or_to_a_link_is_refused_with_the_walk_sentences() {
    let repo = Repo::new("links");
    repo.write("in.txt", "a\n");
    let outside = unique_directory("links-target");
    std::fs::create_dir(&outside).expect("outside dir");
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
        let reply = content_of(&repo.root, requested);
        assert_eq!(
            reply.status,
            WorkspaceFileContentStatus::Refused,
            "{requested}"
        );
        assert_eq!(reply.content, None, "{requested}");
        let error = reply.error.as_deref().expect("the refusal says why");
        assert!(error.contains(fragment), "{requested}: {error}");
        // The same assertion the no-root list runs: a link refusal that
        // starts naming the path it refused dies here too.
        assert_no_root(error, &repo.root);
        assert!(
            !format!("{reply:?}").contains("outside reached"),
            "{requested} read through the link: {reply:?}"
        );
    }
    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir(&junction);
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn a_missing_file_is_refused_with_the_listing_own_sentence() {
    let repo = Repo::new("missing");
    repo.write("in.txt", "a\n");

    let reply = content_of(&repo.root, "never-existed.txt");

    assert_eq!(reply.status, WorkspaceFileContentStatus::Refused);
    let error = reply.error.as_deref().expect("the refusal says why");
    assert!(error.contains("does not exist"), "{error}");
    assert_eq!(reply.content, None);
    assert_eq!(reply.size, None, "a refusal claims nothing");
}

/// Mutant `m:binary` — hand content back for a binary anyway: the reply
/// must refuse the bytes, not pass a NUL run off as text.
#[test]
fn a_binary_file_comes_back_with_its_stat_and_no_content() {
    let repo = Repo::new("binary");
    let bytes = [0x00u8, 0x01, 0xFF, 0xFE, 0x41];
    repo.write("blob.dat", bytes);

    let reply = content_of(&repo.root, "blob.dat");

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
}

/// The reason the extension list exists: an image's bytes are binary, and
/// without recognizing the spelling they would never be shown at all.
#[test]
fn an_image_is_recognized_by_its_extension_and_carries_base64() {
    let repo = Repo::new("image");
    let bytes = [0x89u8, b'P', b'N', b'G', 0x00, 0xFF, 0x0D, 0x0A];
    repo.write("shot.png", bytes);

    let reply = content_of(&repo.root, "shot.png");

    assert_eq!(reply.status, WorkspaceFileContentStatus::Ok);
    assert_eq!(reply.kind, Some(WorkspaceFileContentKind::Image));
    let expected = base64::engine::general_purpose::STANDARD.encode(bytes);
    assert_eq!(reply.content.as_deref(), Some(expected.as_str()));
    assert_eq!(reply.size, Some(8));
}

/// The two things a refusal sentence must never carry: this run's workspace
/// root, and any path-shaped text at all (`/`, `\`, `:` — which catches a
/// leaked *relative* path too). One assertion, two callers: the no-root list
/// below and the link test — a `format!` added at either call site dies.
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
/// confined path; the two link refusals run [`assert_no_root`] in their own
/// test below, so all seven are covered and no arm is skipped. `READ_FAILED`
/// is in neither list: a bare constant with no interpolation, on an arm no
/// test can force open (declared in the report).
#[test]
fn no_refusal_sentence_carries_the_workspace_root() {
    let repo = Repo::new("pathless");
    repo.write("src/lib.rs", "//\n");

    let refusals = [
        content_of(&repo.root, ""),
        content_of(&repo.root, "."),
        content_of(&repo.root, "../escape.txt"),
        content_of(&repo.root, "/absolute/escape.txt"),
        content_of(&repo.root, ".git/config"),
        content_of(&repo.root, "never-existed.txt"),
        content_of(&repo.root, "src"),
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
/// Together with the cap test above this is what pins 128 KiB as derived
/// physics rather than a preference.
#[test]
fn the_worst_content_still_fits_one_frame() {
    let repo = Repo::new("frame");
    repo.write("controls.txt", vec![0x01u8; FILE_READ_MAX_BYTES as usize]);

    let reply = content_of(&repo.root, "controls.txt");
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
