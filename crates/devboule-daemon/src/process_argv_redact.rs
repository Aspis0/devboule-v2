//! Argv redaction for the process index: what a tool result may show of
//! a process's command line, and what is masked or capped before it can.
//! `redact_line` runs the same passes over any free-text line a surface
//! is about to show.

/// An argv list is evidence, not a transcript: cap it before it reaches a
/// tool result.
const MAX_ARGS: usize = 32;
/// The per-argument cap, in characters; longer values are truncated with a
/// marker rather than dropped.
const MAX_ARG_CHARS: usize = 512;
/// Flag words, stored without dashes and matched case-insensitively against
/// whatever dash prefix the argument carries (`--token`, `-token`, `--TOKEN`).
const SECRET_FLAGS: &[&str] = &[
    "token",
    "password",
    "passwd",
    "secret",
    "api-key",
    "apikey",
    "auth-token",
    "access-key",
    "authorization",
    "cookie",
    "p",
    "pass",
    "pwd",
    "passphrase",
    "proxy-password",
    "client-secret",
];
/// Flags whose value is `user:password` when it has a colon (`curl -u`,
/// `--user`, `--proxy-user`). A colonless value is only a user name, and
/// `-u` is also an everyday non-credential flag (`python -u`, `docker -u`).
const USER_PAIR_FLAGS: &[&str] = &["u", "user", "proxy-user"];
/// Name words that mark `KEY=value` as a credential: the name is split on
/// `-` and `_`, so `API_KEY`, `AWS_SECRET_ACCESS_KEY` and a bare `password`
/// match while `keyboard` and `MONKEY` do not.
const SECRET_NAME_WORDS: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passwd",
    "pwd",
    "credential",
    "auth",
    "authorization",
    "cookie",
];
/// Endings that make one glued word a credential name on its own
/// (`PGPASSWORD`, `apiKey`): long enough that `MONKEY` and `keyboard` never
/// match.
const SECRET_NAME_SUFFIXES: &[&str] = &[
    "password",
    "passwd",
    "passphrase",
    "secret",
    "token",
    "apikey",
    "credentials",
];

/// Mask two-word credentials (`Bearer abc123`) in a raw command line, before
/// any splitting: splitting destroys the two-word shape the argv redactor's
/// single-arg guard needs, and a short token falls under every length check
/// after it. The word after a scheme word — `bearer`, `basic` or `token`,
/// in any case, standing free or glued behind a `:`/`=` — is masked only
/// when it reads as a credential: a digit, one of `- _ . = + /`, or sixteen
/// characters, so `Bearer of bad news` survives while `Bearer abc123`
/// does not. The exception is a header name ending in `:` whose name is a
/// secret (`authorization`, `proxy-authorization`, any `is_secret_name`):
/// behind it the value is masked whatever it looks like, and a secret
/// header also owns the token right after it. With nothing after it, the
/// scheme word is an ordinary word. The value span is replaced whole,
/// quotes included.
pub(crate) fn mask_scheme_credentials(line: &str) -> String {
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut start: Option<usize> = None;
    for (index, cell) in line.char_indices() {
        if cell.is_whitespace() {
            if let Some(open) = start.take() {
                spans.push((open, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(open) = start {
        spans.push((open, line.len()));
    }
    let mut masked: Vec<(usize, usize)> = Vec::new();
    for (position, &(from, to)) in spans.iter().enumerate() {
        // A secret header owns the token after it, value shape or not.
        if is_secret_header(&line[from..to]) {
            if let Some(&value) = spans.get(position + 1) {
                masked.push(value);
            }
        }
        let Some((scheme_start, scheme_end)) = find_scheme_word(line, from, to) else {
            continue;
        };
        let mut value_start = scheme_end;
        while value_start < to && matches!(line.as_bytes()[value_start], b':' | b'=') {
            value_start += 1;
        }
        let value = if value_start < to {
            (value_start, to)
        } else {
            match spans.get(position + 1) {
                Some(&next) => next,
                None => continue,
            }
        };
        let previous_word = if scheme_start > from {
            Some(&line[from..scheme_start])
        } else {
            position
                .checked_sub(1)
                .and_then(|index| spans.get(index))
                .map(|&(start, end)| &line[start..end])
        };
        // Behind a header name the scheme's value is masked whatever it
        // looks like; elsewhere it must still read as a credential.
        let behind_header = previous_word.is_some_and(is_secret_header);
        if behind_header || looks_like_credential(&line[value.0..value.1]) {
            masked.push(value);
        }
    }
    masked.sort_unstable();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    for (from, to) in masked {
        if from < cursor {
            continue;
        }
        out.push_str(&line[cursor..from]);
        out.push_str("[redacted]");
        cursor = to;
    }
    out.push_str(&line[cursor..]);
    out
}

/// A value that can carry a credential rather than be ordinary prose: it
/// holds a digit or a credential separator, or it is long enough to be a
/// token.
fn looks_like_credential(value: &str) -> bool {
    value
        .chars()
        .any(|cell| cell.is_ascii_digit() || "-_.=+/".contains(cell))
        || value.chars().count() >= 16
}

/// A header name: the word ends in `:` and its name is a secret.
/// `authorization` and `proxy-authorization` split into `is_secret_name`'s
/// own words, so they need no case here.
fn is_secret_header(word: &str) -> bool {
    word.trim_matches(['\'', '"'])
        .strip_suffix(':')
        .is_some_and(is_secret_name)
}

/// Where a scheme word sits inside one whitespace-delimited token, as
/// `(start, end)`: `:` and `=` split the token, so `Authorization:Bearer`
/// and `--header="Bearer"` still show the scheme word whole. `None` when
/// the token holds none.
fn find_scheme_word(line: &str, from: usize, to: usize) -> Option<(usize, usize)> {
    const SCHEMES: [&str; 3] = ["bearer", "basic", "token"];
    let mut part_start = from;
    for index in from..=to {
        if index == to || matches!(line.as_bytes()[index], b':' | b'=') {
            let word = line[part_start..index].trim_matches(['\'', '"']);
            if SCHEMES.contains(&word.to_ascii_lowercase().as_str()) {
                return Some((part_start, index));
            }
            part_start = index + 1;
        }
    }
    None
}

/// One free-text line with credentials masked: the scheme pass runs on the
/// raw string first — splitting would destroy the two-word shape — then
/// the line is split the way the process probe splits one, with shell
/// quotes off, through the argv redactor.
pub(crate) fn redact_line(line: &str) -> String {
    let masked = mask_scheme_credentials(line);
    let bare: String = masked.replace(['\'', '"'], "");
    let argv: Vec<String> = bare.split_whitespace().map(str::to_string).collect();
    redact_argv(&argv).join(" ")
}

/// any case), a glued `-pVALUE`, a `user:password` pair after `-u`/`--user`,
/// the value half of a credential-shaped `name=value`, the value of
/// a credential header (`Authorization: Bearer …`, a cookie, an API-key
/// header), a bare `Bearer`/`Basic` credential, credentials embedded in a
/// `scheme://user:pass@` URL anywhere in an argument, and any bare
/// token-shaped value.
/// Everything is capped; nothing secret is ever stored or echoed. The
/// heuristics are deliberately one-sided: a false mask costs a bit of
/// evidence, a missed one costs a credential.
pub(crate) fn redact_argv(argv: &[String]) -> Vec<String> {
    let mut redacted: Vec<String> = Vec::new();
    let mut mask_next = false;
    let mut mask_next_pair = false;
    for arg in argv.iter().take(MAX_ARGS) {
        if mask_next {
            redacted.push("[redacted]".to_string());
            mask_next = false;
            continue;
        }
        if std::mem::take(&mut mask_next_pair) && arg.contains(':') {
            redacted.push("[redacted]".to_string());
            continue;
        }
        if let Some((prefix, inline)) = user_pair_flag(arg) {
            match inline {
                Some(value) if value.contains(':') => {
                    redacted.push(format!("{prefix}[redacted]"));
                    continue;
                }
                Some(_) => {}
                None => {
                    mask_next_pair = true;
                    redacted.push(arg.clone());
                    continue;
                }
            }
        }
        if let Some((masked, value_follows)) = mask_header(arg) {
            redacted.push(masked);
            mask_next = value_follows;
            continue;
        }
        if is_auth_scheme_credential(arg) {
            redacted.push("[redacted]".to_string());
            continue;
        }
        let arg = mask_url_credentials(arg);
        let name = arg.split('=').next().unwrap_or(&arg).to_string();
        let secret_flag = name.starts_with('-')
            && SECRET_FLAGS
                .iter()
                .any(|&flag| name.trim_start_matches('-').eq_ignore_ascii_case(flag));
        if secret_flag && arg.contains('=') {
            redacted.push(format!("{name}=[redacted]"));
            continue;
        }
        if secret_flag {
            mask_next = true;
            redacted.push(arg);
            continue;
        }
        if arg.starts_with("-p") && !arg.starts_with("--") && arg.len() > 2 {
            redacted.push("-p[redacted]".to_string());
            continue;
        }
        if arg.contains('=') && is_secret_name(&name) {
            redacted.push(format!("{name}=[redacted]"));
            continue;
        }
        if is_token_shaped(&arg) {
            redacted.push("[redacted]".to_string());
            continue;
        }
        redacted.push(capped(&arg, MAX_ARG_CHARS));
    }
    redacted
}

/// The executable string of a process, redacted like one argument: on macOS
/// it is the first token of `ps`'s rendered command line, which the process
/// chose itself, so `API_KEY=secret` can be spelled there.
pub(crate) fn redact_exe(exe: &str) -> String {
    redact_argv(&[exe.to_string()])
        .pop()
        .unwrap_or_else(|| "[redacted]".to_string())
}

/// `name=value` is a credential when the name splits into secret words or a
/// word ends in one — one-sided on purpose (`file-key` and `PGPASSWORD`
/// mask, `keyboard` and `MONKEY` do not).
fn is_secret_name(name: &str) -> bool {
    name.split(&['-', '_'][..]).any(|word| {
        let word = word.to_ascii_lowercase();
        SECRET_NAME_WORDS.contains(&word.as_str())
            || SECRET_NAME_SUFFIXES
                .iter()
                .any(|suffix| word.ends_with(suffix))
    })
}

/// A `-u`, `--user` or `--proxy-user` argument: what precedes its value
/// (`--user=`, `-u`) and the value when it is glued on. `None` when the
/// argument is not such a flag.
fn user_pair_flag(arg: &str) -> Option<(String, Option<&str>)> {
    if let Some(long) = arg.strip_prefix("--") {
        let (name, value) = match long.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (long, None),
        };
        let listed = USER_PAIR_FLAGS.contains(&name.to_ascii_lowercase().as_str());
        return (listed && name.len() > 1).then(|| (format!("--{name}="), value));
    }
    let short = arg.strip_prefix('-')?;
    let mut letters = short.chars();
    let letter = letters.next()?;
    if !letter.eq_ignore_ascii_case(&'u') {
        return None;
    }
    let value = letters.as_str();
    Some((format!("-{letter}"), (!value.is_empty()).then_some(value)))
}

/// A `Name: value` header argument (a bare one, or the value of
/// `--header=…`) whose name carries a secret word keeps its name and loses
/// its value. The second half is true when the header ended at the colon,
/// which leaves its value in the next argument.
fn mask_header(arg: &str) -> Option<(String, bool)> {
    let (prefix, header) = match arg.strip_prefix('-').and(arg.split_once('=')) {
        Some((flag, rest)) => (format!("{flag}="), rest),
        None => (String::new(), arg),
    };
    let (name, value) = header.split_once(':')?;
    let name = name.trim();
    let header_shaped = !name.is_empty()
        && name
            .chars()
            .all(|cell| cell.is_ascii_alphanumeric() || cell == '-' || cell == '_');
    // `://` is a URL, not a header.
    if !header_shaped || value.starts_with("//") || !is_secret_name(name) {
        return None;
    }
    Some((
        format!("{prefix}{name}: [redacted]"),
        value.trim().is_empty(),
    ))
}

/// An argument that is itself an `Authorization` value: `Bearer …`,
/// `Basic …` or `Token …`, split from its header name by the shell.
fn is_auth_scheme_credential(arg: &str) -> bool {
    let Some((scheme, credential)) = arg.split_once(' ') else {
        return false;
    };
    !credential.trim().is_empty()
        && ["bearer", "basic", "token"]
            .iter()
            .any(|&word| scheme.eq_ignore_ascii_case(word))
}

/// Every `scheme://userinfo@host/...` in a value keeps its scheme and host
/// and loses its userinfo — `user:pass` or a bare token — whatever else the
/// argument holds (`--db=…`, a comma list, a whole shell command).
fn mask_url_credentials(value: &str) -> String {
    let mut masked = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(scheme_end) = rest.find("://") {
        let authority_start = scheme_end + 3;
        masked.push_str(&rest[..authority_start]);
        let tail = &rest[authority_start..];
        let authority_len = tail
            .find(|cell: char| {
                matches!(cell, '/' | '?' | '#' | ',' | '\'' | '"') || cell.is_whitespace()
            })
            .unwrap_or(tail.len());
        let authority = &tail[..authority_len];
        match authority.rfind('@') {
            Some(at) => {
                masked.push_str("[redacted]@");
                masked.push_str(&authority[at + 1..]);
            }
            None => masked.push_str(authority),
        }
        rest = &tail[authority_len..];
    }
    masked.push_str(rest);
    masked
}

/// A token-shaped bare value: long, high-charset, not a path — the kind of
/// argument that sits beside a CLI with no flag to key off. One-sided: it may
/// mask a random-looking id, and it never lets a credential through.
fn is_token_shaped(value: &str) -> bool {
    value.len() >= 24
        && !value.starts_with('/')
        && !value.starts_with('.')
        && value
            .chars()
            .all(|cell| cell.is_ascii_alphanumeric() || "+/=_-".contains(cell))
}

fn capped(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(max).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
#[path = "process_argv_redact_tests.rs"]
mod tests;
