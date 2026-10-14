//! The managed Codex home: what it holds, who may delete it, and what a host
//! with no credentials file does.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::{create, remove_for_session, sweep};

/// A temp runtime dir removed when the test ends, so a run leaves no home
/// behind for the next one to find.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        Self(crate::test_dirs::test_temp_dir(&format!(
            "devboule-codex-home-{tag}"
        )))
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("the parent of a fixture file");
        }
        std::fs::write(&path, content).expect("a fixture file is written");
        path
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A stand-in for the human's own Codex home. Credentials live here and nowhere
/// else, so a copy can be asserted byte-for-byte without reading the machine's
/// real `~/.codex/auth.json`.
fn credentials_home(tag: &str, auth: Option<&str>) -> TempDir {
    let home = TempDir::new(tag);
    if let Some(auth) = auth {
        home.write("auth.json", auth);
    }
    home
}

#[test]
fn a_created_home_carries_its_own_session_marker_and_no_rollout() {
    let runtime = TempDir::new("marker");
    let human = credentials_home("marker-auth", Some("{\"token\":\"secret\"}"));
    let home = create(runtime.path(), "s.home.1", human.path()).expect("the home is created");
    assert!(home.starts_with(runtime.path()), "under the runtime dir");
    assert_eq!(
        std::fs::read_to_string(home.join("devboule-managed-home")).expect("the marker"),
        "s.home.1\n",
        "the marker names the row this home belongs to"
    );
    assert!(
        !home.join("config.toml").exists(),
        "the human's own config never crosses over"
    );
}

#[test]
fn a_second_spawn_reuses_the_row_home_and_refreshes_the_credentials() {
    let runtime = TempDir::new("reuse");
    let human = credentials_home("reuse-auth", Some("{\"token\":\"first\"}"));
    let first = create(runtime.path(), "s.reuse.1", human.path()).expect("the first home");
    // The child may have rotated its own token inside the home; the next spawn
    // starts from the human's file, not from what the last child wrote.
    std::fs::write(first.join("auth.json"), "{\"token\":\"rotated\"}").expect("a child rewrote it");
    human.write("auth.json", "{\"token\":\"second\"}");
    let second = create(runtime.path(), "s.reuse.1", human.path()).expect("the second home");
    assert_eq!(second, first, "the same row runs in the same home");
    assert_eq!(
        std::fs::read_to_string(second.join("auth.json")).expect("the copied auth"),
        "{\"token\":\"second\"}",
        "every spawn imports the credentials as they are now"
    );
}

#[test]
fn another_rows_home_is_never_handed_over_or_touched() {
    let runtime = TempDir::new("two-rows");
    let human = credentials_home("two-rows-auth", Some("{\"token\":\"secret\"}"));
    let mine = create(runtime.path(), "s.mine.1", human.path()).expect("this row's home");
    let theirs = create(runtime.path(), "s.theirs.1", human.path()).expect("the other home");
    remove_for_session(runtime.path(), "s.mine.1");
    assert!(!mine.exists(), "the row that was deleted loses its home");
    assert!(
        theirs.join("devboule-managed-home").exists(),
        "another session's home is none of this one's business"
    );
}

#[test]
fn a_real_home_without_credentials_still_gets_a_usable_home() {
    // A host that authenticates through OPENAI_API_KEY has no auth.json to
    // copy; refusing the spawn there would break the one case where the child
    // was never going to read the file.
    let runtime = TempDir::new("no-auth");
    let human = credentials_home("no-auth-src", None);
    let home = create(runtime.path(), "s.noauth.1", human.path()).expect("the home is created");
    assert!(!home.join("auth.json").exists(), "nothing was invented");
    assert!(
        home.join("devboule-managed-home").exists(),
        "the marker is written either way"
    );
}

#[test]
fn a_directory_this_daemon_never_marked_is_never_deleted() {
    let runtime = TempDir::new("foreign");
    // Our name, no marker of ours: a removal that only looked at the file name
    // would take this, and whatever a person keeps in the runtime dir, with it.
    let unmarked = runtime.path().join("codex-home-not-ours");
    runtime.write("codex-home-not-ours/payload.txt", "not ours");
    // A marker naming a row that still exists is that row's home, not ours to
    // take — the sweep decides on the journal, not on the directory.
    let claimed = runtime.path().join("codex-home-other-row");
    std::fs::create_dir_all(&claimed).expect("a directory under our prefix");
    std::fs::write(claimed.join("devboule-managed-home"), "s.someone.else\n")
        .expect("a marker naming another row");

    remove_for_session(runtime.path(), "s.mine.1");
    assert_eq!(
        sweep(
            runtime.path(),
            &HashSet::from(["s.someone.else".to_string()])
        ),
        0,
        "nothing here is reclaimable: one directory is not ours, the other is still claimed"
    );
    assert!(
        unmarked.join("payload.txt").exists() && claimed.exists(),
        "a path this daemon never claimed stays exactly where it is"
    );
}

#[test]
fn the_sweep_keeps_the_home_a_row_can_still_resume_from() {
    let runtime = TempDir::new("sweep");
    let human = credentials_home("sweep-auth", Some("{\"token\":\"secret\"}"));
    let kept = create(runtime.path(), "s.kept.1", human.path()).expect("a claimed home");
    let swept = create(runtime.path(), "s.gone.1", human.path()).expect("an orphaned home");
    let claimed = HashSet::from(["s.kept.1".to_string()]);
    assert_eq!(sweep(runtime.path(), &claimed), 1, "one home reclaimed");
    assert!(
        kept.exists(),
        "a row that can still be resumed keeps its rollout"
    );
    assert!(!swept.exists(), "a home no row claims is reclaimable");
}

#[test]
fn a_home_whose_marker_is_gone_is_kept_with_its_credentials() {
    let runtime = TempDir::new("marker-guard");
    let human = credentials_home("guard-auth", Some("{\"token\":\"secret\"}"));
    let home = create(runtime.path(), "s.guard.1", human.path()).expect("the home is created");
    std::fs::remove_file(home.join("devboule-managed-home")).expect("the marker is taken away");
    remove_for_session(runtime.path(), "s.guard.1");
    assert!(
        home.join("auth.json").exists(),
        "without the marker the home is kept, credentials and all"
    );
}
