//! The pure half: every rule here is driven from a literal string or a
//! plain temp file — no `git` process and no repository. The cases that need
//! a real repository live in `workspace_git_diff_tests.rs`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use devboule_protocol::WorkspaceGitDiffLineKind;

use super::{parse_diff, path_record, untracked_file, PathRecord, UntrackedFile};

fn unique_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-workspace-diff-parse-{label}-{}-{stamp}",
        std::process::id()
    ))
}

/// Every line of a hunk, by the marker git prints: the `@@` header whole,
/// the content lines with exactly one `+`/`-`/space stripped, the preamble
/// and git's own metadata line (`\ No newline …`) not content at all.
#[test]
fn hunk_lines_take_their_kind_from_gits_own_markers() {
    let stdout = concat!(
        "diff --git a/f.txt b/f.txt\n",
        "index 1111111..2222222 100644\n",
        "--- a/f.txt\n",
        "+++ b/f.txt\n",
        "@@ -1,3 +1,4 @@\n",
        " one\n",
        "-two\n",
        "+tw+o\n",
        "  indented\n",
        "\n",
        "\\ No newline at end of file\n",
    );
    let parsed = parse_diff(stdout);

    assert!(!parsed.is_new && !parsed.is_deleted && !parsed.binary);
    let rendered: Vec<(&str, &str)> = parsed
        .lines
        .iter()
        .map(|line| match line.kind {
            WorkspaceGitDiffLineKind::Header => ("header", line.text.as_str()),
            WorkspaceGitDiffLineKind::Add => ("add", line.text.as_str()),
            WorkspaceGitDiffLineKind::Remove => ("remove", line.text.as_str()),
            WorkspaceGitDiffLineKind::Context => ("context", line.text.as_str()),
        })
        .collect();
    assert_eq!(
        rendered,
        [
            ("header", "@@ -1,3 +1,4 @@"),
            ("context", "one"),
            ("remove", "two"),
            ("add", "tw+o"),
            // A context line of two spaces keeps its inner space: only the
            // marker is stripped.
            ("context", " indented"),
            // The literal spells this empty line bare; a real hunk prints
            // it as a single space. Both are the same context line, and
            // neither may be dropped.
            ("context", ""),
        ],
        "{rendered:?}"
    );
}

/// The preamble is git's verdict, not content: `new file mode` and the
/// `/dev/null` half of `---` mean the path is not in `HEAD`; `deleted file
/// mode` and `+++ /dev/null` mean it is gone from the working tree; a mode
/// change alone is neither.
#[test]
fn the_preamble_flags_say_new_and_deleted() {
    let new = parse_diff(concat!(
        "diff --git a/x b/x\n",
        "new file mode 100644\n",
        "index 0000000..2222222\n",
        "--- /dev/null\n",
        "+++ b/x\n",
        "@@ -0,0 +1,1 @@\n",
        "+line\n",
    ));
    assert!(new.is_new && !new.is_deleted, "{new:?}");

    let deleted = parse_diff(concat!(
        "diff --git a/x b/x\n",
        "deleted file mode 100644\n",
        "index 1111111..0000000\n",
        "--- a/x\n",
        "+++ /dev/null\n",
        "@@ -1,1 +0,0 @@\n",
        "-line\n",
    ));
    assert!(!deleted.is_new && deleted.is_deleted, "{deleted:?}");

    let mode_only = parse_diff(concat!(
        "diff --git a/x b/x\n",
        "old mode 100644\n",
        "new mode 100755\n",
    ));
    assert!(!mode_only.is_new && !mode_only.is_deleted, "{mode_only:?}");
    assert!(mode_only.lines.is_empty());
}

/// git's own sentence for a binary: no hunks follow it, and the reply must
/// carry `binary` rather than an empty `ok` that reads as "no changes".
#[test]
fn a_binary_preamble_is_binary_and_carries_no_lines() {
    let parsed = parse_diff(concat!(
        "diff --git a/blob.dat b/blob.dat\n",
        "index c94be36..8caf516 100644\n",
        "Binary files a/blob.dat and b/blob.dat differ\n",
    ));
    assert!(parsed.binary, "{parsed:?}");
    assert!(parsed.lines.is_empty());
}

/// `git status --porcelain=v2 -z -- <path>`: only the kind of the first
/// record matters — the pathspec already scoped the dump, and a rename's
/// original path is the bare record that follows, which must never be
/// classified by position.
#[test]
fn path_record_reads_only_the_kind_of_the_first_record() {
    let cases: [(&str, Option<&str>); 7] = [
        ("? fresh.txt\0", Some("untracked")),
        (
            "1 .M N... 100644 100644 100644 a b f.txt\0",
            Some("tracked"),
        ),
        (
            "2 R. N... 100644 100644 100644 a b R100 new.txt\0old.txt\0",
            Some("tracked"),
        ),
        (
            "u UU N... 100644 100644 100644 100644 a a b c conflict.txt\0",
            Some("tracked"),
        ),
        ("# branch.head main\0", None),
        ("nonsense\0", None),
        ("", None),
    ];
    for (stdout, expected) in cases {
        let classified = match path_record(stdout) {
            Some(PathRecord::Untracked) => Some("untracked"),
            Some(PathRecord::Tracked) => Some("tracked"),
            None => None,
        };
        assert_eq!(classified, expected, "for {stdout:?}");
    }
}

/// The synthesis of an untracked file: every line an addition, no phantom
/// line for the terminator, and an empty file is an empty diff rather than
/// one empty line.
#[test]
fn synthesized_lines_are_additions_and_the_terminator_is_not_a_line() {
    let file = unique_path("synth");
    let lines_of = |contents: &str| {
        std::fs::write(&file, contents).expect("write");
        match untracked_file(&file) {
            UntrackedFile::Lines(lines) => lines,
            _ => panic!("expected lines for {contents:?}"),
        }
    };

    let two = lines_of("a\nb\n");
    assert_eq!(two.len(), 2);
    assert!(two
        .iter()
        .all(|line| line.kind == WorkspaceGitDiffLineKind::Add));
    assert_eq!(
        two.iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        ["a", "b"]
    );

    let no_terminator = lines_of("a");
    assert_eq!(no_terminator.len(), 1, "{no_terminator:?}");
    assert_eq!(no_terminator[0].text, "a");

    let trailing_blank = lines_of("a\n\n");
    assert_eq!(trailing_blank.len(), 2, "{trailing_blank:?}");

    let empty = lines_of("");
    assert!(empty.is_empty(), "an empty file is not one empty line");
    let _ = std::fs::remove_file(&file);
}

/// A NUL byte is git's own definition of binary — checked before any text
/// is handed out — and a file that cannot be opened has no lines to give.
#[test]
fn a_nul_byte_is_binary_and_an_unopenable_file_says_so() {
    let file = unique_path("binary");
    std::fs::write(&file, b"a\n\0b\n").expect("write");
    assert!(matches!(untracked_file(&file), UntrackedFile::Binary));
    let _ = std::fs::remove_file(&file);

    assert!(
        matches!(untracked_file(&file), UntrackedFile::Unreadable),
        "gone is not zero lines, and not an empty diff either"
    );
}

/// Mutant `m:i` — keep the carriage return. `parse_diff` gets its lines
/// from `str::lines()`, which strips the `\r` of a CRLF file from git's own
/// output; the synthesis must strip it too, or the same file reads with a
/// trailing `\r` on every line when untracked and without it when git
/// printed the diff — two renderings of one file in one panel.
#[test]
fn synthesized_lines_strip_the_carriage_return_like_gits_own_output() {
    let file = unique_path("crlf");
    std::fs::write(&file, b"first\r\nsecond\r\n").expect("write");
    let lines = match untracked_file(&file) {
        UntrackedFile::Lines(lines) => lines,
        _ => panic!("a CRLF text file is lines"),
    };
    let _ = std::fs::remove_file(&file);

    assert_eq!(
        lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert!(
        lines.iter().all(|line| !line.text.contains('\r')),
        "a carriage return survived the synthesis: {:?}",
        lines
    );
}
