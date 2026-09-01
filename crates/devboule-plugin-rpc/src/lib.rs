//! Host↔plugin-backend conversation over the same protocol frames as the daemon.
//!
//! The plugin backend binds a named pipe, the host connects, they handshake
//! with plugin-scoped capabilities, and the host sends [`devboule_protocol::ClientMessage::Invoke`].
//! Process membership is a Windows Job Object with `KILL_ON_JOB_CLOSE` so an
//! orphan cannot outlive the host. Tested by killing the backend mid-request.

mod error;
mod pipe;
mod server;
mod session;
mod spawn;

pub use error::PluginError;
pub use server::{unix_millis, PluginBackend};
pub use session::{method_is_granted, workspace_root_from_value, PluginSession, SpawnSpec};
pub use spawn::{
    pipe_name_from_env_or_argv, sha256_file, unique_pipe_name, verify_file_digest, HOST_PID_ENV,
    PIPE_ENV, PLUGIN_ID_ENV,
};

use devboule_protocol::{caps, Capability, OwnerId};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

// These are the filesystem redirectors. Other reparse tags, including the
// cloud-file and HSM tags, describe filesystem metadata/providers rather than
// a path redirect and are therefore not rejected by confinement.
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
#[cfg(test)]
const IO_REPARSE_TAG_CLOUD: u32 = 0x9000_001A;
#[cfg(test)]
const IO_REPARSE_TAG_HSM: u32 = 0xC000_0004;

fn is_redirecting_reparse_tag(tag: u32) -> bool {
    matches!(tag, IO_REPARSE_TAG_SYMLINK | IO_REPARSE_TAG_MOUNT_POINT)
}

/// Advance the host's ownership token when a lease is reacquired. Wrapping is
/// intentional: equality is the only validity check and zero is a valid token.
pub fn next_generation(current: u64) -> u64 {
    current.wrapping_add(1)
}

#[cfg(windows)]
fn reparse_tag(path: &Path) -> Result<Option<u32>, String> {
    use std::iter::once;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const FSCTL_GET_REPARSE_POINT: u32 = 589_992;
    const ERROR_NOT_A_REPARSE_POINT: u32 = 4390;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(once(0)).collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            null(),
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(format!(
            "cannot inspect reparse tag for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }

    let mut buffer = [0u8; 16_384];
    let mut bytes_returned = 0u32;
    let ok = unsafe {
        DeviceIoControl(
            handle,
            FSCTL_GET_REPARSE_POINT,
            null(),
            0,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut bytes_returned,
            null_mut(),
        )
    };
    let error = if ok == 0 {
        Some(unsafe { GetLastError() })
    } else {
        None
    };
    unsafe {
        CloseHandle(handle);
    }
    if let Some(error) = error {
        if error == ERROR_NOT_A_REPARSE_POINT {
            return Ok(None);
        }
        return Err(format!(
            "cannot inspect reparse tag for {}: OS error {error}",
            path.display()
        ));
    }
    if bytes_returned < 4 {
        return Err(format!(
            "cannot inspect reparse tag for {}: truncated result",
            path.display()
        ));
    }
    Ok(Some(u32::from_le_bytes(
        buffer[..4].try_into().expect("four-byte tag"),
    )))
}

/// Capabilities the host is willing to grant this plugin, given what the
/// manifest requested and the confined workspace root (if any).
pub fn granted_capabilities(
    requested: &[String],
    workspace_root: Option<&str>,
) -> (Vec<Capability>, BTreeMap<String, String>) {
    let mut capabilities = vec![Capability::new(caps::PING)];
    let mut grants = BTreeMap::new();
    let requested_root = requested.iter().any(|name| name == caps::WORKSPACE_ROOT);
    if requested_root {
        if let Some(root) = workspace_root {
            capabilities.push(Capability::new(caps::WORKSPACE_ROOT));
            grants.insert(caps::WORKSPACE_ROOT.to_string(), root.to_string());
            if requested.iter().any(|name| name == caps::CITY_GET) {
                capabilities.push(Capability::new(caps::CITY_GET));
            }
        }
    }
    (capabilities, grants)
}

pub fn host_owner() -> Result<OwnerId, PluginError> {
    #[cfg(windows)]
    {
        let user = devboule_daemon::current_user_sid()?;
        OwnerId::new(user, "devboule-app").map_err(PluginError::Protocol)
    }
    #[cfg(not(windows))]
    {
        OwnerId::new("unix", "devboule-app").map_err(PluginError::Protocol)
    }
}

/// Canonicalize the active project root without following an attacker-selected
/// symlink or accepting a path that climbs through a parent component.
pub fn confine_project_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("project root must be absolute".to_string());
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err("project root contains parent traversal".to_string());
    }

    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)
            .map_err(|error| format!("project root cannot be inspected: {error}"))?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                if let Some(tag) = reparse_tag(ancestor)? {
                    if is_redirecting_reparse_tag(tag) {
                        return Err(format!(
                            "project root contains a redirecting reparse point (tag 0x{tag:08X}): {}",
                            ancestor.display()
                        ));
                    }
                }
            }
        }
        #[cfg(not(windows))]
        if metadata.file_type().is_symlink() {
            return Err(format!(
                "project root contains a symlink: {}",
                ancestor.display()
            ));
        }
        if ancestor.parent().is_none() {
            break;
        }
    }

    let canonical = std::fs::canonicalize(path)
        .map_err(|error| format!("project root cannot be canonicalized: {error}"))?;
    if !canonical.is_dir() {
        return Err("project root is not a directory".to_string());
    }
    Ok(canonical)
}

/// Convert a canonical Windows path into the ordinary path form expected by
/// plugin consumers. `canonicalize` may return the extended-length spelling;
/// only the transport spelling is changed, never the path that was confined.
pub fn workspace_root_for_grant(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
        rest.to_string()
    } else {
        text.into_owned()
    }
}

#[cfg(test)]
mod path_tests {
    use super::*;

    #[test]
    fn workspace_grants_strip_windows_verbatim_prefixes() {
        assert_eq!(
            workspace_root_for_grant(Path::new(r"\\?\C:\repo")),
            r"C:\repo"
        );
        assert_eq!(
            workspace_root_for_grant(Path::new(r"\\?\UNC\server\share\repo")),
            r"\\server\share\repo"
        );
        assert_eq!(workspace_root_for_grant(Path::new(r"C:\repo")), r"C:\repo");
    }

    #[test]
    fn reacquiring_a_session_advances_its_generation() {
        assert_eq!(next_generation(5), 6);
        assert_eq!(next_generation(u64::MAX), 0);
    }

    #[test]
    fn reparse_tag_policy_allows_cloud_tags_but_rejects_redirectors() {
        assert!(is_redirecting_reparse_tag(IO_REPARSE_TAG_SYMLINK));
        assert!(is_redirecting_reparse_tag(IO_REPARSE_TAG_MOUNT_POINT));
        assert!(!is_redirecting_reparse_tag(IO_REPARSE_TAG_CLOUD));
        assert!(!is_redirecting_reparse_tag(IO_REPARSE_TAG_HSM));
    }

    #[test]
    fn project_root_is_canonical_and_parent_traversal_is_refused() {
        let directory = tempfile::tempdir().expect("temp directory");
        let root = directory.path().join("project");
        std::fs::create_dir(&root).expect("project directory");

        let confined = confine_project_path(&root).expect("project root");
        assert_eq!(
            confined,
            std::fs::canonicalize(&root).expect("canonical root")
        );

        let traversal = root.join("..").join("project");
        let error = confine_project_path(&traversal).expect_err("parent traversal");
        assert!(error.contains("parent traversal"), "{error}");

        let link = directory.path().join("project-link");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&root, &link).is_ok();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&root, &link).is_ok();
        if linked {
            let error = confine_project_path(&link).expect_err("symlink root");
            assert!(
                error.contains("symlink") || error.contains("reparse"),
                "{error}"
            );
        }

        #[cfg(windows)]
        {
            let outside = directory.path().join("outside-project");
            std::fs::create_dir(&outside).expect("outside project directory");
            let junction = directory.path().join("project-junction");
            let junction_created = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&junction)
                .arg(&outside)
                .status()
                .map(|status| status.success())
                .unwrap_or(false);
            if junction_created {
                let error = confine_project_path(&junction).expect_err("junction root");
                assert!(
                    error.contains("redirecting") || error.contains("reparse"),
                    "{error}"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_root_is_granted_only_when_requested_and_known() {
        let (caps, grants) = granted_capabilities(&["workspace.root".into()], Some(r"C:\repo"));
        assert!(caps.iter().any(|cap| cap.as_str() == "workspace.root"));
        assert_eq!(
            grants.get("workspace.root").map(String::as_str),
            Some(r"C:\repo")
        );

        let (caps, grants) = granted_capabilities(&["workspace.root".into()], None);
        assert!(!caps.iter().any(|cap| cap.as_str() == "workspace.root"));
        assert!(grants.is_empty());

        let (caps, grants) = granted_capabilities(
            &["workspace.root".into(), "city.get".into()],
            Some(r"C:\repo"),
        );
        assert!(caps.iter().any(|cap| cap.as_str() == "city.get"));
        assert_eq!(grants.get("workspace.root").map(String::as_str), Some(r"C:\repo"));

        let (caps, grants) = granted_capabilities(&["city.get".into()], Some(r"C:\repo"));
        assert!(!caps.iter().any(|cap| cap.as_str() == "city.get"));
        assert!(grants.is_empty());

        let (caps, grants) = granted_capabilities(&["oracle.search".into()], Some(r"C:\repo"));
        assert!(!caps.iter().any(|cap| cap.as_str() == "workspace.root"));
        assert!(grants.is_empty());
    }
}
