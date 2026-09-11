//! Shell-wrapper unwrapping for `commandExecution` titles.
//!
//! Exact port of Paseo's
//! `packages/server/src/server/agent/providers/codex/tool-call-mapper.ts`
//! (`unwrapShellCommand`, `stripMatchingEdgeQuotes`,
//! `normalizeCommandExecutionCommand`, including the array form and
//! `isWindowsShellCommand`). The title keeps our convention (bare command,
//! label separate): no "Run command:" prefix.
//!
//! Paseo's originals, quoted:
//!
//! ```typescript
//! function unwrapShellCommand(command: string): string {
//!   const trimmed = command.trim();
//!   const unix = trimmed.match(/^(?:(?:\/[^/\s]+)*\/)?(?:zsh|bash|sh)\s+-(?:lc|c)\s+([\s\S]+)$/);
//!   if (unix?.[1]) return stripMatchingEdgeQuotes(unix[1].trim());
//!   const windows = trimmed.match(
//!     /^(?:"[^"]*\\)?(?:pwsh|powershell|cmd)(?:\.exe)?"?(?:\s+-[A-Za-z]+(?:\s+[^-\s][^\s]*)?)*\s+(?:-Command|-c|\/c)\s+([\s\S]+)$/i,
//!   );
//!   return windows?.[1] ? stripMatchingEdgeQuotes(windows[1].trim()) : trimmed;
//! }
//!
//! function stripMatchingEdgeQuotes(value: string): string {
//!   if (
//!     (value.startsWith('"') && value.endsWith('"')) ||
//!     (value.startsWith("'") && value.endsWith("'"))
//!   ) {
//!     return value.slice(1, -1);
//!   }
//!   return value;
//! }
//!
//! function isWindowsShellCommand(command: string): boolean {
//!   const normalized = command.replace(/^["']|["']$/g, "");
//!   return /(?:^|\\)(?:pwsh|powershell|cmd)(?:\.exe)?$/i.test(normalized);
//! }
//! ```
//!
//! Exactness: the string branches below implement Paseo's two anchored
//! regexes rule-for-rule over byte offsets into the trimmed input. The
//! payload is the raw remainder after the flag (`([\s\S]+)$`), never
//! tokenized or re-joined, so interior whitespace and newlines survive. The
//! Windows command flag is the rightmost viable one, mirroring the greedy
//! option run with backtracking. Whitespace throughout is JavaScript `\s`
//! without the `u` flag (see `is_js_space`); trimming uses the same set,
//! exactly like `String.prototype.trim`.
//!
//! Deviations from Paseo: none.

use serde_json::Value;

/// JavaScript `\s` as matched by Paseo's patterns (no `u` flag): TAB LF VT
/// FF CR SPACE plus the Unicode spaces below. This deliberately differs from
/// Rust's `char::is_whitespace` in two places: U+0085 is not matched, and
/// U+FEFF is.
fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            | '\u{2001}'
            | '\u{2002}'
            | '\u{2003}'
            | '\u{2004}'
            | '\u{2005}'
            | '\u{2006}'
            | '\u{2007}'
            | '\u{2008}'
            | '\u{2009}'
            | '\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202F}'
            | '\u{205F}'
            | '\u{3000}'
            | '\u{FEFF}'
    )
}

/// JavaScript `String.prototype.trim`: its set equals `is_js_space`.
fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_space)
}

/// Strip one pair of matching edge quotes (`"..."` or `'...'`). Mismatched
/// edges are left alone, exactly like Paseo's `stripMatchingEdgeQuotes`
/// (which likewise has no length guard: a lone quote strips to `""`).
pub(crate) fn strip_matching_edge_quotes(value: &str) -> String {
    let bytes = value.as_bytes();
    // Paseo has no length guard: a lone quote character strips to `""`.
    if bytes.len() == 1 && (bytes[0] == b'"' || bytes[0] == b'\'') {
        return String::new();
    }
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

/// Byte index just past the whitespace run starting at `i`. Callers pass
/// char boundaries (0, ASCII arithmetic, token edges, or previous results).
fn skip_js_space(s: &str, mut i: usize) -> usize {
    while let Some(c) = s[i..].chars().next() {
        if !is_js_space(c) {
            break;
        }
        i += c.len_utf8();
    }
    i
}

/// Split `s[from..]` on JavaScript-whitespace runs, returning byte ranges.
/// Paseo has no quote awareness anywhere in these patterns: quotes never
/// protect whitespace.
fn split_ws_ranges(s: &str, from: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (rel, c) in s[from..].char_indices() {
        let i = from + rel;
        if is_js_space(c) {
            if let Some(st) = start.take() {
                out.push((st, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        out.push((st, s.len()));
    }
    out
}

/// Case-sensitive interpreter name at byte index `i` (Paseo's unix branch
/// has no `i` flag); returns the end index. First letters are disjoint, so
/// alternation order is moot. `starts_with` never panics on boundaries.
fn unix_name_end(s: &str, i: usize) -> Option<usize> {
    for name in ["zsh", "bash", "sh"] {
        if s[i..].starts_with(name) {
            return Some(i + name.len());
        }
    }
    None
}

/// Paseo's `-(?:lc|c)`, with `lc` tried first. The caller requires
/// whitespace after, which makes the token exact.
fn unix_flag_end(s: &str, i: usize) -> Option<usize> {
    if s[i..].starts_with("-lc") {
        return Some(i + 3);
    }
    if s[i..].starts_with("-c") {
        return Some(i + 2);
    }
    None
}

/// Paseo's unix branch:
/// `^(?:(?:\/[^/\s]+)*\/)?(?:zsh|bash|sh)\s+-(?:lc|c)\s+([\s\S]+)$`.
/// Name-start candidates are tried longest-prefix first (the greedy optional
/// prefix unwinds from the back): segment ends, then the lone `/` (zero
/// segments plus the trailing slash also matches), then no prefix.
fn match_unix(s: &str) -> Option<String> {
    let mut starts = vec![0usize];
    if s.starts_with('/') {
        // The lone `/`: tried after longer prefixes, before no prefix.
        starts.push(1);
        let mut pos = 0usize; // byte index of a '/'
        loop {
            let mut j = pos + 1;
            while let Some(c) = s[j..].chars().next() {
                if c == '/' || is_js_space(c) {
                    break;
                }
                j += c.len_utf8();
            }
            if j == pos + 1 {
                break; // empty segment: `//`, `/ `, or end of input
            }
            if s[j..].starts_with('/') {
                starts.push(j + 1);
                pos = j;
            } else {
                break;
            }
        }
    }
    for i in starts.iter().rev() {
        let i = *i;
        if let Some(e) = unix_name_end(s, i) {
            let w = skip_js_space(s, e);
            if w == e {
                continue;
            }
            if let Some(f) = unix_flag_end(s, w) {
                let p = skip_js_space(s, f);
                if p == f || p >= s.len() {
                    continue;
                }
                return Some(strip_matching_edge_quotes(js_trim(&s[p..])).to_string());
            }
        }
    }
    None
}

/// Paseo's `(?:-Command|-c|\/c)` with the `i` flag.
fn is_cmd_flag(token: &str) -> bool {
    token.eq_ignore_ascii_case("-command")
        || token.eq_ignore_ascii_case("-c")
        || token.eq_ignore_ascii_case("/c")
}

/// Paseo's `-[A-Za-z]+`, matched against the entire token.
fn is_option_token(token: &str) -> bool {
    token.len() >= 2
        && token.starts_with('-')
        && token[1..].bytes().all(|b| b.is_ascii_alphabetic())
}

/// Whether `toks` parses completely as Paseo's option run: each option,
/// greedily followed by exactly one value token when the next token does
/// not start with `-` (Paseo's `(?:\s+[^-\s][^\s]*)?`). The segmentation is
/// forced — an option-shaped token is always an option, a non-dash token is
/// always a value — so no search is needed here; the only search is over the
/// flag position in `find_payload_start`.
fn parses_as_option_run(s: &str, toks: &[(usize, usize)]) -> bool {
    let mut k = 0;
    while k < toks.len() {
        let (a, b) = toks[k];
        if !is_option_token(&s[a..b]) {
            return false;
        }
        k += 1;
        if k < toks.len() {
            let (c, d) = toks[k];
            if !s[c..d].starts_with('-') {
                k += 1;
            }
        }
    }
    true
}

const WINDOWS_NAMES: [&str; 3] = ["pwsh", "powershell", "cmd"];

/// Whether `\` + NAME [+ `.exe`] (case-insensitive) ends at byte index
/// `end`, at or after index 1 (the opening quote for the quoted form).
/// Existence only; the caller decides which end wins.
fn quoted_name_ends_at(s: &str, end: usize) -> bool {
    if !s.starts_with('"') {
        return false;
    }
    for name in WINDOWS_NAMES {
        // Either form counts here; the caller only needs existence.
        for exe in [".exe", ""] {
            let tail_len = 1 + name.len() + exe.len();
            if tail_len > end || end > s.len() {
                continue;
            }
            let Some(slot) = s.get(end - tail_len..end) else {
                continue;
            };
            let mut want = String::with_capacity(tail_len);
            want.push('\\');
            want.push_str(name);
            want.push_str(exe);
            if slot.len() == tail_len && slot.eq_ignore_ascii_case(&want) {
                return true;
            }
        }
    }
    false
}

/// Candidate interpreter ends in Paseo's backtracking order. For a leading
/// `"`, the closing quote is consumed first (`"?` is greedy); names ending
/// before the closing quote with whitespace after come later, rightmost end
/// first. For a bare name, `.exe` comes before bare.
fn interpreter_ends(s: &str) -> Vec<usize> {
    if let Some(quoted) = s.strip_prefix('"') {
        let Some(close) = quoted.find('"').map(|rel| rel + 1) else {
            return Vec::new();
        };
        let mut ends = Vec::new();
        if quoted_name_ends_at(s, close) {
            ends.push(close + 1);
        }
        let mut extra: Vec<usize> = Vec::new();
        for (rel, c) in s[2..close].char_indices() {
            let e = 2 + rel;
            if is_js_space(c) && quoted_name_ends_at(s, e) {
                extra.push(e);
            }
        }
        extra.sort_unstable_by(|a, b| b.cmp(a));
        ends.extend(extra);
        ends
    } else {
        for name in WINDOWS_NAMES {
            let Some(head) = s.get(..name.len()) else {
                continue;
            };
            if !head.eq_ignore_ascii_case(name) {
                continue;
            }
            let mut ends = Vec::new();
            if s[name.len()..]
                .get(..4)
                .is_some_and(|ext| ext.eq_ignore_ascii_case(".exe"))
            {
                ends.push(name.len() + 4);
            }
            ends.push(name.len());
            return ends
                .into_iter()
                .map(|e| {
                    // Paseo's `"?`: consuming a following quote comes first;
                    // the empty variant is doomed there (`"` is not
                    // whitespace) and is skipped.
                    if s[e..].starts_with('"') {
                        e + 1
                    } else {
                        e
                    }
                })
                .collect();
        }
        Vec::new()
    }
}

/// The payload starts after the rightmost viable flag: the rightmost token
/// that is `-Command`/`-c`/`/c` (case-insensitive) with at least one token
/// after it, such that every token between the interpreter and it parses as
/// an option run. Returns the byte index into the raw string (never
/// re-joined tokens), mirroring the greedy option run with backtracking.
fn find_payload_start(s: &str, from: usize) -> Option<usize> {
    // Paseo requires `\s+` after the interpreter before the first option
    // or the flag: a glued interpreter (`pwsh-Command x`) never matches.
    if skip_js_space(s, from) == from {
        return None;
    }
    let toks = split_ws_ranges(s, from);
    for i in (0..toks.len()).rev() {
        let (a, b) = toks[i];
        if !is_cmd_flag(&s[a..b]) {
            continue;
        }
        if i + 1 >= toks.len() {
            continue;
        }
        if !parses_as_option_run(s, &toks[..i]) {
            continue;
        }
        let after = skip_js_space(s, b);
        if after >= s.len() {
            continue;
        }
        return Some(after);
    }
    None
}

/// Paseo's windows branch:
/// `^(?:"[^"]*\\)?(?:pwsh|powershell|cmd)(?:\.exe)?"?(?:\s+-[A-Za-z]+(?:\s+[^-\s][^\s]*)?)*\s+(?:-Command|-c|\/c)\s+([\s\S]+)$`
/// with the `i` flag.
fn match_windows(s: &str) -> Option<String> {
    for end in interpreter_ends(s) {
        if let Some(pstart) = find_payload_start(s, end) {
            return Some(strip_matching_edge_quotes(js_trim(&s[pstart..])).to_string());
        }
    }
    None
}

/// Paseo's `unwrapShellCommand`: the payload of a shell wrapper, or the
/// trimmed input when it is not one. Unix is tried first, as in Paseo.
pub(crate) fn unwrap_shell_command(command: &str) -> String {
    let trimmed = js_trim(command);
    if trimmed.is_empty() {
        return String::new();
    }
    if let Some(unwrapped) = match_unix(trimmed) {
        return unwrapped;
    }
    if let Some(unwrapped) = match_windows(trimmed) {
        return unwrapped;
    }
    trimmed.to_string()
}

/// Paseo's `isWindowsShellCommand`, literally: strip one leading and one
/// trailing quote of either type (`/^["']|["']$/g`), then test
/// `(?:^|\\)(?:pwsh|powershell|cmd)(?:\.exe)?$` case-insensitively
/// (backslash only, never forward slash).
fn is_windows_shell_command(command: &str) -> bool {
    let v = command.strip_prefix(['"', '\'']).unwrap_or(command);
    let v = v.strip_suffix(['"', '\'']).unwrap_or(v);
    let base = match v.rfind('\\') {
        Some(i) => &v[i + 1..],
        None => v,
    };
    WINDOWS_NAMES.iter().any(|name| {
        if base.len() < name.len() {
            return false;
        }
        match base.get(..name.len()) {
            Some(head) if head.eq_ignore_ascii_case(name) => {
                let rest = &base[name.len()..];
                rest.is_empty() || rest.eq_ignore_ascii_case(".exe")
            }
            _ => false,
        }
    })
}

/// Paseo's `normalizeCommandExecutionCommand`: strings unwrap; arrays
/// starting with a shell plus `-lc`/`-c` (unix, flag only, no quote strip on
/// the payload) or a Windows shell command plus `-Command`/`-c`/`/c` unwrap
/// to the payload; any other array joins with spaces.
pub(crate) fn normalize_command_execution_command(value: &Value) -> Option<String> {
    if let Some(command) = value.as_str() {
        let normalized = unwrap_shell_command(command);
        return (!normalized.is_empty()).then_some(normalized);
    }
    let array = value.as_array()?;
    let parts: Vec<String> = array
        .iter()
        .filter_map(Value::as_str)
        .map(js_trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect();
    if parts.is_empty() {
        return None;
    }
    if parts.len() >= 3 {
        if parts[1] == "-lc" || parts[1] == "-c" {
            let unwrapped = js_trim(&parts[2]);
            return (!unwrapped.is_empty()).then(|| unwrapped.to_string());
        }
        if is_windows_shell_command(&parts[0]) && is_cmd_flag(&parts[1]) {
            let joined = parts[2..].join(" ");
            let unwrapped = js_trim(&joined);
            return (!unwrapped.is_empty())
                .then(|| strip_matching_edge_quotes(unwrapped).to_string());
        }
    }
    Some(parts.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(command: &str) -> Value {
        Value::String(command.to_string())
    }

    #[test]
    fn unix_shell_wrappers_unwrap_to_the_payload() {
        assert_eq!(
            normalize_command_execution_command(&value("bash -lc \"ls -la\"")).as_deref(),
            Some("ls -la")
        );
        assert_eq!(
            normalize_command_execution_command(&value("/bin/zsh -c 'echo hi'")).as_deref(),
            Some("echo hi")
        );
        assert_eq!(
            normalize_command_execution_command(&value("sh -c ls")).as_deref(),
            Some("ls")
        );
    }

    #[test]
    fn measured_pwsh_wrapper_unwraps_to_the_payload() {
        // Measured wire (fixtures/wire/codex/E1-step1-handshake.jsonl):
        // `"C:\\Users\\gualt\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe" -Command 'git status'`.
        let measured = value(
            "\"C:\\Users\\gualt\\AppData\\Local\\Microsoft\\WindowsApps\\pwsh.exe\" -Command 'git status'",
        );
        assert_eq!(
            normalize_command_execution_command(&measured).as_deref(),
            Some("git status")
        );
        assert_eq!(
            normalize_command_execution_command(&value(
                "powershell -NoProfile -Command \"Get-ChildItem\""
            ))
            .as_deref(),
            Some("Get-ChildItem")
        );
        assert_eq!(
            normalize_command_execution_command(&value("cmd /c dir")).as_deref(),
            Some("dir")
        );
    }

    #[test]
    fn plain_commands_pass_through_unchanged() {
        assert_eq!(
            normalize_command_execution_command(&value("git status")).as_deref(),
            Some("git status")
        );
    }

    #[test]
    fn array_form_unwraps_shell_and_flag() {
        let array = serde_json::json!(["bash", "-lc", "ls"]);
        assert_eq!(
            normalize_command_execution_command(&array).as_deref(),
            Some("ls")
        );
    }

    #[test]
    fn mismatched_edge_quotes_are_not_stripped() {
        assert_eq!(
            normalize_command_execution_command(&value("'echo hi\"")).as_deref(),
            Some("'echo hi\"")
        );
    }

    #[test]
    fn payload_whitespace_is_preserved_verbatim() {
        // Paseo captures `([\\s\\S]+)$` raw: no tokenization, no re-join.
        assert_eq!(
            normalize_command_execution_command(&value("bash -c \"echo \\\"hi\\\"\"")).as_deref(),
            Some("echo \\\"hi\\\"")
        );
        assert_eq!(
            normalize_command_execution_command(&value("bash -c 'echo   hi'")).as_deref(),
            Some("echo   hi")
        );
        assert_eq!(
            normalize_command_execution_command(&value("bash -c echo    hello")).as_deref(),
            Some("echo    hello")
        );
        assert_eq!(
            normalize_command_execution_command(&value("bash -c echo\nfoo")).as_deref(),
            Some("echo\nfoo")
        );
    }

    #[test]
    fn unix_branch_is_case_sensitive_with_no_exe_or_backslash_forms() {
        for command in ["BASH -c ls", "bash.exe -c 'ls'", "C:\\bin\\bash -c 'ls'"] {
            assert_eq!(
                normalize_command_execution_command(&value(command)).as_deref(),
                Some(command),
                "Paseo leaves {command} unchanged"
            );
        }
    }

    #[test]
    fn unquoted_windows_paths_do_not_unwrap() {
        let command =
            "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe -Command \"x\"";
        assert_eq!(
            normalize_command_execution_command(&value(command)).as_deref(),
            Some(command)
        );
    }

    #[test]
    fn quoted_windows_path_with_spaces_unwraps() {
        assert_eq!(
            normalize_command_execution_command(&value("\"C:\\my tools\\pwsh.exe\" -Command ls"))
                .as_deref(),
            Some("ls")
        );
    }

    #[test]
    fn windows_flag_is_the_rightmost_viable_one() {
        // Greedy option run with backtracking: an earlier flag is consumed
        // as an option and the later one wins.
        assert_eq!(
            normalize_command_execution_command(&value("pwsh -c -Command 'ls'")).as_deref(),
            Some("ls")
        );
        assert_eq!(
            normalize_command_execution_command(&value("pwsh -c \"a\" -Command \"b\"")).as_deref(),
            Some("b")
        );
        // `/c` is leftmost-viable here: `echo` is not an option.
        assert_eq!(
            normalize_command_execution_command(&value("cmd /c echo -Command x")).as_deref(),
            Some("echo -Command x")
        );
    }

    #[test]
    fn windows_options_must_match_dash_letters_entirely() {
        assert_eq!(
            normalize_command_execution_command(&value("pwsh -x1 -Command 'ls'")).as_deref(),
            Some("pwsh -x1 -Command 'ls'")
        );
    }

    #[test]
    fn quoted_option_values_do_not_span_spaces() {
        // Paseo's value token `[^\\-\\s][^\\s]*` cannot span the space, so the
        // `-Command` never becomes viable.
        let command = "pwsh -File \"C:\\my script.ps1\" -Command x";
        assert_eq!(
            normalize_command_execution_command(&value(command)).as_deref(),
            Some(command)
        );
    }

    #[test]
    fn unix_prefix_backtracking_prefers_the_longest_viable_name() {
        assert_eq!(
            normalize_command_execution_command(&value("/sh/sh -c x")).as_deref(),
            Some("x")
        );
        assert_eq!(
            normalize_command_execution_command(&value("//sh -c x")).as_deref(),
            Some("//sh -c x")
        );
    }

    #[test]
    fn unix_lone_slash_prefix_unwraps() {
        // The optional prefix `(?:(?:\/[^/\s]+)*\/)?` also matches a lone
        // `/` with zero segments.
        assert_eq!(
            normalize_command_execution_command(&value("/bash -c x")).as_deref(),
            Some("x")
        );
        assert_eq!(
            normalize_command_execution_command(&value("/sh -lc \"ls\"")).as_deref(),
            Some("ls")
        );
    }

    #[test]
    fn windows_interpreter_must_be_followed_by_whitespace() {
        // After the interpreter (`(?:\.exe)?"?`) the regex requires `\s+`
        // before the first option or the flag.
        for command in [
            "pwsh-Command x",
            "pwsh.exe\"-c x",
            "\"C:\\t\\pwsh.exe\"-Command x",
        ] {
            assert_eq!(
                normalize_command_execution_command(&value(command)).as_deref(),
                Some(command),
                "Paseo leaves {command} unchanged"
            );
        }
        assert_eq!(
            normalize_command_execution_command(&value("pwsh -Command x")).as_deref(),
            Some("x")
        );
    }

    #[test]
    fn missing_payloads_and_case_variants() {
        for command in ["bash -c", "pwsh -Command"] {
            assert_eq!(
                normalize_command_execution_command(&value(command)).as_deref(),
                Some(command)
            );
        }
        assert_eq!(
            normalize_command_execution_command(&value("PWSH -Command ls")).as_deref(),
            Some("ls")
        );
        assert_eq!(
            normalize_command_execution_command(&value("CMD /C dir")).as_deref(),
            Some("dir")
        );
        // A whitespace-only payload strips to nothing visible (Paseo quirk,
        // verified against node): trim keeps the quotes, strip removes them.
        assert_eq!(
            normalize_command_execution_command(&value("pwsh -Command \" \"")).as_deref(),
            Some(" ")
        );
    }

    #[test]
    fn array_windows_interpreter_uses_literal_quote_rules() {
        // `isWindowsShellCommand`: strip one leading/trailing quote of either
        // type, then `(?:^|\\\\)(?:pwsh|powershell|cmd)(?:\\.exe)?$`.
        for (array, expected) in [
            (serde_json::json!(["\"pwsh\"", "-Command", "ls"]), "ls"),
            (serde_json::json!(["'pwsh'", "-Command", "ls"]), "ls"),
            (
                serde_json::json!(["C:/tools/pwsh.exe", "-Command", "ls"]),
                "C:/tools/pwsh.exe -Command ls",
            ),
            (
                serde_json::json!(["C:\\tools\\pwsh.exe", "-Command", "ls"]),
                "ls",
            ),
        ] {
            assert_eq!(
                normalize_command_execution_command(&array).as_deref(),
                Some(expected),
                "array {array}"
            );
        }
    }
}
