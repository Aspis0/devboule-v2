//! The macOS trust step for a staged Chrome for Testing bundle.
//!
//! UNVERIFIED on this machine: there is no Mac here, so this compiled on
//! Windows only and never ran. The macOS CI job compiles it and exercises the
//! order; a human on a Mac must watch it refuse a tampered bundle before this
//! header comes off.
//!
//! The order is the whole of this module: the signature seal, then the
//! bundle's own designated requirement, then Gatekeeper with the download's
//! quarantine untouched. Quarantine is removed only from the verified bundle
//! and only after a Gatekeeper refusal that leaving it in place explains; a
//! bundle that is still refused afterwards fails the install. The bytes are
//! never re-signed or mutated. Google documents the `xattr` remedy for
//! browser-downloaded ZIPs; Apple documents Gatekeeper/notarization.

#![cfg_attr(not(test), allow(dead_code))]

use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Output};

/// Google's Developer ID team, the one Chrome for Testing is signed by: the
/// bundle's designated requirement must state it, or the bytes belong to
/// someone else. Published by the Chromium commit that records the
/// requirement; cited in the slice report.
const GOOGLE_TEAM: &str = "EQHXZ8M8AV";

/// The bundle identifier measured in the pinned mac-arm64 archive's
/// `Info.plist`, so an install cannot be satisfied by another Google app.
const CFT_BUNDLE_ID: &str = "com.google.chrome.for.testing";

/// The signed thing: the app bundle around the binary, not the binary.
#[cfg(target_os = "macos")]
const BUNDLE: &str = "Google Chrome for Testing.app";

/// Whether a `codesign --display --requirements -` answer describes the
/// Chrome for Testing bundle signed by Google's team. Quotes are dropped
/// first: the requirement prints identifiers and the team id either way.
fn requirement_is_google(display: &str) -> bool {
    let plain = display.replace('"', "");
    plain.contains(&format!("identifier {CFT_BUNDLE_ID}"))
        && plain.contains("anchor apple generic")
        && plain.contains(&format!("subject.OU] = {GOOGLE_TEAM}"))
}

/// What a Gatekeeper answer means for the quarantine attribute.
#[derive(Debug, PartialEq, Eq)]
enum Quarantine {
    /// Gatekeeper accepted: leave the attribute exactly as it arrived.
    Keep,
    /// Gatekeeper refused and the attribute is present: the documented xattr
    /// remedy is the only remaining step, on this verified bundle alone.
    Strip,
    /// Refused with nothing to strip: report Gatekeeper's own answer.
    Refuse,
}

fn quarantine_verdict(accepted: bool, quarantined: bool) -> Quarantine {
    match (accepted, quarantined) {
        (true, _) => Quarantine::Keep,
        (false, true) => Quarantine::Strip,
        (false, false) => Quarantine::Refuse,
    }
}

/// Verify the staged bundle and clear its quarantine so Gatekeeper lets the
/// app-owned copy launch. `staged` is the version directory the install is
/// about to rename into place, so the path stripped is always the verified
/// versioned bundle and nothing else.
#[cfg(target_os = "macos")]
pub(super) fn trust(staged: &Path) -> Result<(), String> {
    let bundle = staged.join(BUNDLE);
    let verified = run("codesign", &["--verify", "--deep", "--strict"], &bundle)?;
    if !verified.status.success() {
        return Err(format!(
            "the macOS signature check refused the browser: {}",
            refusal(&verified)
        ));
    }
    let requirement = run("codesign", &["--display", "--requirements", "-"], &bundle)?;
    let display = format!(
        "{}{}",
        String::from_utf8_lossy(&requirement.stdout),
        String::from_utf8_lossy(&requirement.stderr)
    );
    if !requirement_is_google(&display) {
        return Err(format!(
            "the browser's designated requirement is not Google's: {}",
            display.trim()
        ));
    }
    let accepted = run("spctl", &["--assess", "--type", "execute"], &bundle)?;
    match quarantine_verdict(accepted.status.success(), quarantined(&bundle)?) {
        Quarantine::Keep => Ok(()),
        Quarantine::Refuse => Err(format!(
            "Gatekeeper refused the browser: {}",
            refusal(&accepted)
        )),
        Quarantine::Strip => {
            let stripped = run("xattr", &["-dr", "com.apple.quarantine"], &bundle)?;
            if !stripped.status.success() {
                return Err(format!(
                    "the quarantine could not be removed: {}",
                    refusal(&stripped)
                ));
            }
            let after = run("spctl", &["--assess", "--type", "execute"], &bundle)?;
            if after.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "Gatekeeper refused the browser even after the quarantine was removed: {}",
                    refusal(&after)
                ))
            }
        }
    }
}

/// Off macOS there is no signature to check and no quarantine to strip: the
/// staged copy is trusted by its SHA-256 alone.
#[cfg(not(target_os = "macos"))]
pub(super) fn trust(_staged: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn run(tool: &str, args: &[&str], bundle: &Path) -> Result<Output, String> {
    Command::new(tool)
        .args(args)
        .arg(bundle)
        .output()
        .map_err(|error| format!("{tool} did not run: {error}"))
}

/// Whether the download left a quarantine attribute on the bundle.
#[cfg(target_os = "macos")]
fn quarantined(bundle: &Path) -> Result<bool, String> {
    Command::new("xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(bundle)
        .output()
        .map(|answer| answer.status.success())
        .map_err(|error| format!("xattr did not run: {error}"))
}

/// The tool's own words, stderr first because that is where both `codesign`
/// and `spctl` put the reason.
#[cfg(target_os = "macos")]
fn refusal(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    text.trim().to_owned()
}

#[cfg(test)]
#[path = "cft_macos_tests.rs"]
mod tests;
