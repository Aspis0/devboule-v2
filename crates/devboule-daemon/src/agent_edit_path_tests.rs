//! Where a stored edit path lands in the workspace: inside the root it is
//! root-relative, and any path that leaves the root is refused.

use std::path::Path;

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
