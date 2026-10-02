//! The plain spelling an editor's command line receives for a canonical path.
//! The function is pure, so every case runs on every platform.

use crate::backend::editor_path_spelling::plain_spelling;

#[test]
fn a_disk_path_loses_the_extended_prefix() {
    assert_eq!(
        plain_spelling(r"\\?\C:\Users\Zoë Ünï\my repo\src\main.rs"),
        r"C:\Users\Zoë Ünï\my repo\src\main.rs"
    );
    assert_eq!(plain_spelling(r"\\?\D:\"), r"D:\");
}

#[test]
fn a_unc_path_becomes_the_two_backslash_form() {
    assert_eq!(
        plain_spelling(r"\\?\UNC\server\share\repo\a.rs"),
        r"\\server\share\repo\a.rs"
    );
}

#[test]
fn a_path_that_is_already_plain_is_untouched() {
    for plain in [
        r"C:\ws\src\main.rs",
        r"\\server\share\a.rs",
        "/home/me/a.rs",
    ] {
        assert_eq!(plain_spelling(plain), plain);
    }
}

#[test]
fn a_path_that_would_pass_max_path_keeps_the_canonical_form() {
    // MAX_PATH counts the terminating NUL: 259 characters fit, 260 do not.
    let fits = format!(r"\\?\C:\{}", "d".repeat(256));
    assert_eq!(plain_spelling(&fits), format!(r"C:\{}", "d".repeat(256)));

    let long = format!(r"\\?\C:\{}", "d".repeat(257));
    assert_eq!(plain_spelling(&long), long);
}

#[test]
fn a_form_that_is_neither_disk_nor_unc_keeps_the_canonical_form() {
    for odd in [
        r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\x",
        r"\\?\GLOBALROOT\Device\HarddiskVolume1\x",
        r"\\?\C:",
        r"\\?\UNC\server",
        r"\\?\UNC\\share\x",
    ] {
        assert_eq!(plain_spelling(odd), odd);
    }
}

#[test]
fn a_name_only_the_extended_form_can_spell_keeps_the_canonical_form() {
    for odd in [
        r"\\?\C:\ws\notes.",
        r"\\?\C:\ws\notes ",
        r"\\?\C:\ws\con",
        r"\\?\C:\ws\NUL.txt",
        r"\\?\C:\ws\com1",
        r"\\?\UNC\server\share\lpt9.log",
    ] {
        assert_eq!(plain_spelling(odd), odd);
    }
    assert_eq!(plain_spelling(r"\\?\C:\ws\console.rs"), r"C:\ws\console.rs");
}
