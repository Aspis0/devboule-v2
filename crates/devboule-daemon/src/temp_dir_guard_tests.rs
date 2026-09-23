//! Guard: outside `test_dirs.rs`, no scanned line may carry the system
//! temp dir in a spelling this rule matches: the token
//! (`std::env::temp_dir()`, `env::temp_dir()`), a `use` line importing
//! `temp_dir` from `std::env`, or a bare `temp_dir()` call in a file that
//! carries such an import.
//! Prose is skipped: lines starting `//`, `/*` or `*`, and a trailing
//! `//` comment after the code (stripped quote-aware, so a `//` inside a
//! string literal stays). String literals are otherwise not parsed — a
//! literal spelling the token on a code line is red on purpose. NOT seen
//! by this rule, declared so nobody mistakes it for closure:
//! `use std::env as e` + `e::temp_dir()`, `env::var("TEMP")`,
//! `use std::env::*`, a token split by whitespace (`env :: temp_dir`), and
//! a `/* … */` block whose interior line begins with prose. Production
//! sites that may keep the token sit in `ALLOW`, keyed by relative path +
//! the allowed line's own text: a shifted line stays exempt, another line
//! cannot borrow the exemption, and an entry matching nothing fails the
//! guard. Test code gets no entry — a test that needs a path takes it
//! from the helper.

use std::path::{Path, PathBuf};

/// The helper, as its path under `src` — exemption by relative path, so a
/// nested `test_dirs.rs` deeper in the tree stays in scope.
const HELPER: &str = "test_dirs.rs";

/// Production sites that may keep the token: relative path + the exact
/// text of the allowed line. `concat!` spells it so this file's own scan
/// does not meet the token in its own source; test code has no entries.
const ALLOW: &[(&str, &str)] = &[(
    "login_shell_env.rs",
    concat!("let directory = std::env", "::temp_dir();"),
)];

#[test]
fn no_code_line_outside_the_helper_asks_for_the_temp_dir() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let token = ["env", "temp_dir"].join("::");
    let mut violations = Vec::new();
    let mut used = vec![false; ALLOW.len()];
    walk(&src, &src, &token, &mut violations, &mut used);
    for (index, (path, snippet)) in ALLOW.iter().enumerate() {
        if !used[index] {
            violations.push(format!(
                "stale allowlist entry ({path}): `{snippet}` matched no violation line"
            ));
        }
    }
    assert!(
        violations.is_empty(),
        "code outside {HELPER} asking for the system temp dir:\n{}",
        violations.join("\n")
    );
}

fn walk(dir: &Path, src: &Path, token: &str, violations: &mut Vec<String>, used: &mut [bool]) {
    let entries = std::fs::read_dir(dir).expect("read the src tree");
    for entry in entries {
        let path: PathBuf = entry.expect("an entry").path();
        if path.is_dir() {
            walk(&path, src, token, violations, used);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            check(&path, src, token, violations, used);
        }
    }
}

fn check(path: &Path, src: &Path, token: &str, violations: &mut Vec<String>, used: &mut [bool]) {
    let Ok(relative) = path.strip_prefix(src) else {
        return;
    };
    let site_file = relative.to_string_lossy().replace('\\', "/");
    if site_file == HELPER {
        return;
    }
    let text = std::fs::read_to_string(path).expect("read a source file");
    let lines: Vec<&str> = text.lines().collect();
    let imports_temp = lines.iter().any(|line| {
        !is_comment_line(line.trim_start()) && imports_env_temp_dir(strip_trailing_comment(line))
    });
    for (index, raw) in lines.iter().enumerate() {
        if is_comment_line(raw.trim_start()) {
            continue;
        }
        let code = strip_trailing_comment(raw);
        let reason = if code.contains(token) {
            Some("asks for the system temp dir")
        } else if imports_env_temp_dir(code) {
            Some("imports temp_dir from std::env")
        } else if imports_temp && has_bare_temp_dir_call(code) {
            Some("calls the bare temp_dir() this file imports")
        } else {
            None
        };
        let Some(reason) = reason else {
            continue;
        };
        if let Some(slot) = ALLOW
            .iter()
            .position(|(path, snippet)| *path == site_file && code.contains(snippet))
        {
            used[slot] = true;
            continue;
        }
        violations.push(format!("{}:{} — {reason}", site_file, index + 1));
    }
}

/// A line the scan treats as prose before any matching: a `//` line, a
/// `/*` opener, or a `*` continuation (which also covers the `*/` closer).
fn is_comment_line(trimmed: &str) -> bool {
    trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with("*")
}

/// Everything before the first `//` outside a string literal, so prose
/// after the code is skipped while a `//` inside a string stays code.
/// Byte-wise on purpose: ASCII `//` and `"` never occur inside a UTF-8
/// continuation byte, so the slice index is always a char boundary.
fn strip_trailing_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
        } else if *byte == b'"' {
            in_string = true;
        } else if *byte == b'/' && bytes.get(index + 1) == Some(&b'/') {
            return &line[..index];
        }
    }
    line
}

/// A `use` line pulling `temp_dir` out of `std::env`, in either spelling.
/// The non-brace form carries the full token and is caught by that check
/// too; this keeps the import visible for the bare-call rule below.
fn imports_env_temp_dir(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("use ")
        && (trimmed.contains("std::env::{") || trimmed.contains("std::env::"))
        && trimmed.contains("temp_dir")
}

/// `temp_dir(` as its own call — not `test_temp_dir(`, not `env::temp_dir(`
/// (a `:` before it), not a mention without parens.
fn has_bare_temp_dir_call(line: &str) -> bool {
    let mut rest = line;
    while let Some(found) = rest.find("temp_dir") {
        let before_ok = rest[..found]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == ':'));
        let after = rest[found + "temp_dir".len()..].trim_start();
        if before_ok && after.starts_with('(') {
            return true;
        }
        rest = &rest[found + "temp_dir".len()..];
    }
    false
}

/// One assertion per comment shape the scan must treat as prose, plus the
/// string-literal invariant the quote-aware strip exists to keep. The token
/// is assembled, never written contiguously — this file is scanned too.
#[test]
fn comment_shapes_are_prose_and_a_slash_in_a_string_is_code() {
    let token = ["env", "temp_dir"].join("::");
    let trailing = format!("let dir = here(); // {token} lives in prose");
    assert!(!strip_trailing_comment(&trailing).contains(&token));
    let opener = format!("/* {token} */");
    assert!(is_comment_line(opener.trim_start()));
    let continuation = format!(" * {token} inside a block");
    assert!(is_comment_line(continuation.trim_start()));
    let in_string = format!("let url = \"http://x\"; let d = {token}();");
    assert!(strip_trailing_comment(&in_string).contains(&token));
}
