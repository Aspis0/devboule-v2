//! The NSIS template's pin guard.
//!
//! The template is a fork of tauri-bundler's `installer.nsi`, so a Tauri CLI
//! bump moves the file underneath it. `Cargo.lock` cannot carry that pin:
//! tauri-bundler is not a dependency of this repo, it lives inside
//! `@tauri-apps/cli`'s prebuilt binary, so the guard watches the CLI version
//! `pnpm-lock.yaml` resolves. The bundler version in the template header is
//! what the person re-diffing has to look up.

/// The pinned template, verbatim in this crate's source tree.
const TEMPLATE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/nsis/installer.nsi");
/// The lock that resolves `@tauri-apps/cli`, the CLI `pnpm tauri build` runs.
const LOCK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../pnpm-lock.yaml");

/// The version prefix of `text`: `"2.11.4, whose …"` -> `"2.11.4"`.
fn version_token(text: &str) -> &str {
    let end = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    &text[..end]
}

fn template_cli_pin(template: &str) -> String {
    template
        .lines()
        .take(8)
        .find_map(|line| line.split_once("tauri-cli "))
        .map(|(_, rest)| version_token(rest))
        .filter(|version| !version.is_empty())
        .expect("the template header states the tauri-cli version it was pinned from")
        .to_string()
}

fn locked_cli_version(lock: &str) -> String {
    lock.lines()
        .find_map(|line| line.trim().strip_prefix("'@tauri-apps/cli@"))
        .map(version_token)
        .filter(|version| !version.is_empty())
        .expect("pnpm-lock.yaml resolves @tauri-apps/cli")
        .to_string()
}

#[test]
fn the_template_header_matches_the_locked_cli() {
    let template = std::fs::read_to_string(TEMPLATE).expect("src-tauri/nsis/installer.nsi");
    let lock = std::fs::read_to_string(LOCK).expect("the repo's pnpm-lock.yaml");
    let pinned = template_cli_pin(&template);
    let locked = locked_cli_version(&lock);
    assert_eq!(
        pinned, locked,
        "src-tauri/nsis/installer.nsi was pinned from tauri-cli {pinned} but pnpm-lock.yaml \
         resolves {locked}: tauri-bundler's installer.nsi lives inside the CLI binary, so \
         re-diff the pinned template against the new bundler's template, then update its header"
    );
}
