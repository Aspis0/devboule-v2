//! The one place a `\\?\`-prefixed path is turned back into its plain
//! spelling. The prefix is a real feature — it is the only spelling that
//! can address a path longer than MAX_PATH — so it is removed only when the
//! plain form names the same path a plain caller would reach.

/// The one place a stored path is turned back into its plain spelling: for
/// the wire, for a person, and for every child process cwd. Stored paths
/// are canonical (`\\?\C:\…`), and the prefix is a real feature — it is the
/// only spelling that can address a path longer than MAX_PATH — so it is
/// removed only when the plain form names the same path a plain caller
/// would reach:
/// - a plain form over `MAX_PATH` keeps the prefix (it is what makes the
///   path usable at all);
/// - a component that is a reserved device name, or that ends with a dot
///   or a space, resolves differently without the prefix (Win32 strips
///   trailing dots and spaces and claims reserved names only in the plain
///   namespace), so stripping would change the meaning;
/// - a `\\?\UNC\` workspace keeps its prefix: a cmd-side child cannot use
///   a UNC cwd either way.
pub(crate) fn plain_path(path: &str) -> String {
    const MAX_PATH: usize = 260;
    const VERBATIM_PREFIX: &str = r"\\?\";
    if path
        .get(..VERBATIM_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(VERBATIM_PREFIX))
        && path
            .get(VERBATIM_PREFIX.len()..)
            .is_some_and(is_verbatim_drive_path)
    {
        let plain = &path[VERBATIM_PREFIX.len()..];
        // Windows counts MAX_PATH in UTF-16 code units plus the terminating
        // NUL, not in bytes: non-ASCII characters cost two UTF-8 bytes each,
        // so a plainly usable path can exceed 260 bytes while staying under
        // 260 units — counting bytes would keep `\\?\` on it and hand a
        // verbatim cwd to cmd.exe.
        if plain.encode_utf16().count() < MAX_PATH && !has_verbatim_only_component(plain) {
            return plain.to_string();
        }
    }
    path.to_string()
}

fn is_verbatim_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

/// A plain path that would resolve differently from its verbatim spelling:
/// **any** component — not just the last — that Win32 treats as a device, or
/// trims, in the plain namespace but takes literally under `\\?\`: a child
/// stripped of the prefix starts somewhere else when `release.` becomes
/// `release` or `CON` is claimed mid-path.
fn has_verbatim_only_component(path: &str) -> bool {
    const RESERVED: [&str; 28] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "COM¹", "COM²", "COM³", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7",
        "LPT8", "LPT9", "LPT¹", "LPT²", "LPT³",
    ];
    let mut seen = false;
    for component in path
        .split(['\\', '/'])
        .filter(|component| !component.is_empty())
    {
        seen = true;
        let name = component
            .split_once('.')
            .map_or(component, |(name, _)| name);
        if RESERVED
            .iter()
            .any(|device| name.eq_ignore_ascii_case(device))
            || component.ends_with('.')
            || component.ends_with(' ')
        {
            return true;
        }
    }
    // No component at all is not a path a plain caller can name.
    !seen
}

#[cfg(test)]
mod tests {
    use super::plain_path;

    /// The review's path: the last component is clean, but `release.` is
    /// normalized away in the plain namespace, so a child stripped of the
    /// prefix would start in `C:\release\repo` while the workspace is
    /// `C:\release.\repo`. Every component has to be checked, not just the
    /// last non-empty one.
    #[test]
    fn plain_path_keeps_the_prefix_when_a_middle_component_is_verbatim_only() {
        assert_eq!(plain_path(r"\\?\C:\release.\repo"), r"\\?\C:\release.\repo");
        assert_eq!(plain_path(r"\\?\C:\release \repo"), r"\\?\C:\release \repo");
        // A reserved device name mid-path, with and without an extension:
        // Win32 claims the stem before the dot anywhere in the plain
        // namespace.
        assert_eq!(
            plain_path(r"\\?\C:\repo\con.txt\src"),
            r"\\?\C:\repo\con.txt\src"
        );
        assert_eq!(plain_path(r"\\?\C:\repo\lpt3\src"), r"\\?\C:\repo\lpt3\src");
        // The superscript device names are not ASCII: the case fold must
        // leave `¹` to compare exactly, mid-path.
        assert_eq!(plain_path(r"\\?\C:\repo\COM¹\src"), r"\\?\C:\repo\COM¹\src");
    }

    /// Windows' MAX_PATH counts UTF-16 code units plus the terminating
    /// NUL, not UTF-8 bytes: this path is 263 bytes but only 133 units,
    /// so the plain spelling is usable and the prefix must come off.
    #[test]
    fn plain_path_measures_the_windows_limit_in_utf16_units_and_the_nul() {
        let plain = format!(r"C:\{}", "é".repeat(130));
        assert!(
            plain.len() >= 260,
            "the fixture is over the byte count: {}",
            plain.len()
        );
        assert_eq!(plain_path(&format!(r"\\?\{plain}")), plain);
    }

    #[test]
    fn plain_path_strips_only_a_short_drive_path_with_plain_safe_components() {
        assert_eq!(
            plain_path(r"\\?\C:\Users\alice\Project"),
            r"C:\Users\alice\Project"
        );
        assert_eq!(plain_path(r"\\?\C:\Project"), r"C:\Project");
        // A plain form over MAX_PATH keeps the prefix: it is the only
        // spelling that addresses the path at all.
        let deep = r"\\?\C:\";
        let long_tail = "word\\";
        let long = format!("{deep}{}", long_tail.repeat(60));
        assert!(plain_path(&long).starts_with(r"\\?\"));
        assert_eq!(plain_path(&long), long);
        // A reserved device name, or a trailing dot or space, resolves
        // differently without the prefix.
        assert_eq!(plain_path(r"\\?\C:\proj\CON"), r"\\?\C:\proj\CON");
        assert_eq!(plain_path(r"\\?\C:\proj\CON.txt"), r"\\?\C:\proj\CON.txt");
        assert_eq!(plain_path(r"\\?\C:\proj\lpt9"), r"\\?\C:\proj\lpt9");
        assert_eq!(plain_path(r"\\?\C:\proj\name."), r"\\?\C:\proj\name.");
        assert_eq!(plain_path(r"\\?\C:\proj\name "), r"\\?\C:\proj\name ");
        // Shapes the prefix rules do not recognize stay untouched, as do
        // paths that are already plain.
        assert_eq!(plain_path(r"\\?\"), r"\\?\");
        assert_eq!(
            plain_path(r"\\?\Volume{abcd}\folder"),
            r"\\?\Volume{abcd}\folder"
        );
        assert_eq!(
            plain_path(r"C:\Users\alice\Project"),
            r"C:\Users\alice\Project"
        );
        assert_eq!(plain_path(r"relative\path"), r"relative\path");
    }
}
