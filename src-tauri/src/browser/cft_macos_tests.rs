//! The macOS trust decision without a Mac: the requirement text and the
//! quarantine rule are strings, so their refusals are testable here while the
//! syscalls that produce them are not.

use super::*;

/// The shape `codesign --display --requirements -` prints for the pinned
/// archive: the bundle id, the anchor and Google's team id in the leaf.
const GOOGLE_REQUIREMENT: &str = "designated => (identifier \"com.google.chrome.for.testing\" or \
     identifier \"com.google.chrome.for.testing.canary\") and anchor apple generic and \
     certificate 1[field.1.2.840.113635.100.6.2.6] /* exists */ and certificate \
     leaf[field.1.2.840.113635.100.6.1.13] /* exists */ and certificate \
     leaf[subject.OU] = EQHXZ8M8AV";

#[test]
fn googles_designated_requirement_is_google() {
    assert!(requirement_is_google(GOOGLE_REQUIREMENT));
    assert!(requirement_is_google(&GOOGLE_REQUIREMENT.replace(
        "subject.OU] = EQHXZ8M8AV",
        "subject.OU] = \"EQHXZ8M8AV\""
    )));
}

#[test]
fn a_requirement_from_another_team_is_refused() {
    let other = GOOGLE_REQUIREMENT.replace("EQHXZ8M8AV", "SOMEONEELSE");
    assert!(!requirement_is_google(&other));
}

#[test]
fn a_requirement_for_another_bundle_is_refused() {
    let other = GOOGLE_REQUIREMENT.replace("com.google.chrome.for.testing", "com.google.Chrome");
    assert!(!requirement_is_google(&other));
}

#[test]
fn a_requirement_without_the_apple_anchor_is_refused() {
    let ad_hoc = GOOGLE_REQUIREMENT.replace("anchor apple generic and ", "");
    assert!(!requirement_is_google(&ad_hoc));
    assert!(!requirement_is_google("designated => cdhash H\"deadbeef\""));
}

#[test]
fn quarantine_is_stripped_only_after_a_refusal_that_the_attribute_explains() {
    assert_eq!(quarantine_verdict(true, true), Quarantine::Keep);
    assert_eq!(quarantine_verdict(true, false), Quarantine::Keep);
    assert_eq!(quarantine_verdict(false, true), Quarantine::Strip);
    assert_eq!(quarantine_verdict(false, false), Quarantine::Refuse);
}
