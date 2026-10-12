//! Where a stored edit path lands in the workspace: inside the root it is
//! root-relative, and any path that leaves the root is refused.

use std::path::{Path, PathBuf};

use super::{path_key, workspace_relative};
use crate::test_support::{fixture_path, FIXTURE_ROOT};

fn root() -> &'static Path {
    Path::new(FIXTURE_ROOT)
}

#[test]
fn a_relative_path_is_root_relative_with_forward_slashes() {
    assert_eq!(
        workspace_relative("src/lib.rs", None, root()).as_deref(),
        Some("src/lib.rs")
    );
}

#[test]
fn a_parent_escape_out_of_the_root_is_refused() {
    assert_eq!(workspace_relative("../secret", None, root()), None);
}

#[test]
fn an_absolute_path_outside_the_root_is_refused() {
    let outside = root()
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt");
    assert_eq!(
        workspace_relative(&outside.to_string_lossy(), None, root()),
        None
    );
}

#[test]
fn a_path_in_a_nested_child_cwd_is_root_relative() {
    let child = fixture_path("sub");
    assert_eq!(
        workspace_relative("a.rs", Some(&child), root()).as_deref(),
        Some("sub/a.rs")
    );
    assert_eq!(
        workspace_relative("../x.rs", Some(&child), root()).as_deref(),
        Some("x.rs")
    );
    assert_eq!(
        workspace_relative(
            &fixture_path("x.rs").to_string_lossy(),
            Some(&child),
            root()
        )
        .as_deref(),
        Some("x.rs")
    );
}

#[test]
fn the_root_itself_is_not_a_file() {
    assert_eq!(
        workspace_relative(&root().to_string_lossy(), None, root()),
        None
    );
}

#[cfg(windows)]
#[test]
fn a_case_variant_on_windows_is_one_key() {
    let upper = workspace_relative("SRC\\LIB.RS", None, root()).expect("inside the root");
    let lower = workspace_relative("src/lib.rs", None, root()).expect("inside the root");
    assert_eq!(path_key(&upper), path_key(&lower));
}

fn update_with(edits: Vec<devboule_protocol::AgentToolEdit>) -> devboule_protocol::SessionEvent {
    devboule_protocol::SessionEvent::AgentToolUpdate {
        tool_call_id: "toolu_confine".to_string(),
        status: Some("completed".to_string()),
        text: None,
        title: None,
        kind: None,
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        replace: false,
        images: Vec::new(),
        edits: Some(edits),
    }
}

fn edit_at(path: &str) -> devboule_protocol::AgentToolEdit {
    devboule_protocol::AgentToolEdit {
        path: path.to_string(),
        kind: devboule_protocol::AgentEditKind::Write,
        text: None,
        truncated: false,
        added: 1,
        removed: 0,
    }
}

fn confined_paths(event: &devboule_protocol::SessionEvent) -> Option<Vec<String>> {
    match event {
        devboule_protocol::SessionEvent::AgentToolUpdate { edits, .. } => edits
            .as_ref()
            .map(|edits| edits.iter().map(|edit| edit.path.clone()).collect()),
        other => panic!("expected a tool update, got {other:?}"),
    }
}

#[test]
fn confine_keeps_the_inside_edits_root_relative_and_drops_the_rest() {
    let scope = super::EditScope {
        cwd: Some(fixture_path("sub")),
        root: Some(PathBuf::from(FIXTURE_ROOT)),
    };
    let outside = PathBuf::from(FIXTURE_ROOT)
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt");
    let mut event = update_with(vec![
        edit_at("a.rs"),
        edit_at("../../secret"),
        edit_at(&outside.to_string_lossy()),
        edit_at(&fixture_path("x.rs").to_string_lossy()),
    ]);
    super::confine_event(&mut event, &scope);
    assert_eq!(
        confined_paths(&event),
        Some(vec!["sub/a.rs".to_string(), "x.rs".to_string()])
    );
}

#[test]
fn confine_with_no_root_drops_every_edit() {
    let scope = super::EditScope {
        cwd: Some(fixture_path("sub")),
        root: None,
    };
    let mut event = update_with(vec![edit_at("a.rs")]);
    super::confine_event(&mut event, &scope);
    assert_eq!(confined_paths(&event), None);
}
