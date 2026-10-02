//! The spelling a canonical Windows path travels in on an editor's command
//! line: `canonicalize` answers in the extended-length form, which the
//! containment comparison wants and a third-party command line may not parse.

/// The terminating NUL counts: a plain path holds at most 259 characters.
const MAX_PATH: usize = 260;

const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

/// `\\?\C:\x` becomes `C:\x` and `\\?\UNC\srv\share\x` becomes
/// `\\srv\share\x`. Any other form, a plain spelling past MAX_PATH, or a name
/// only the extended form can address stays exactly as given.
pub(crate) fn plain_spelling(canonical: &str) -> String {
    let plain = if let Some(rest) = canonical.strip_prefix(r"\\?\UNC\") {
        let mut parts = rest.split('\\');
        let named = |part: Option<&str>| part.is_some_and(|name| !name.is_empty());
        if !(named(parts.next()) && named(parts.next())) {
            return canonical.to_string();
        }
        format!(r"\\{rest}")
    } else if let Some(rest) = canonical.strip_prefix(r"\\?\").filter(|rest| is_disk(rest)) {
        rest.to_string()
    } else {
        return canonical.to_string();
    };
    if plain.encode_utf16().count() < MAX_PATH && plain.split('\\').all(is_plain_name) {
        plain
    } else {
        canonical.to_string()
    }
}

/// A drive letter, its colon, and then a separator.
fn is_disk(rest: &str) -> bool {
    let bytes = rest.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// Win32 strips a trailing dot or space and reads a device name as the
/// device: such a name names another file once the prefix is gone.
fn is_plain_name(name: &str) -> bool {
    if name.ends_with(['.', ' ']) {
        return false;
    }
    let stem = name.split('.').next().unwrap_or(name).trim_end_matches(' ');
    let upper = stem.to_ascii_uppercase();
    let numbered = ["COM", "LPT"].iter().any(|prefix| {
        upper
            .strip_prefix(prefix)
            .is_some_and(|digit| matches!(digit.as_bytes(), [b'1'..=b'9']))
    });
    !numbered && !RESERVED.contains(&upper.as_str())
}
