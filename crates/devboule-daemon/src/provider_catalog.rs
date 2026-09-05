//! Provider discovery for the command-line agents available to Devboule.
//!
//! The six-agent alias table is adapted from herdr, licensed under the Apache
//! License, Version 2.0, commit `3150bd9`, specifically
//! `herdr-src/src/detect/mod.rs:149-218`.
//! The executable-file check is likewise adapted from herdr under that
//! license and commit, from `herdr-src/src/integration/registry.rs:161-179`.
//! The launch resolver is Devboule code: it follows PATHEXT but retains only
//! extensions that `std::process::Command` can launch directly. On Windows it
//! then unwraps an npm `cmd-shim` `.cmd`/`.bat` to `node` plus the package
//! script, so CreateProcess does not go through `cmd.exe`. That unwrap is the
//! inverse of herdr's `normalized_process_name` /
//! `agent_name_from_known_package_path` (`herdr-src/src/detect/mod.rs:359-650`,
//! commit `3150bd9`), which identify a running `node.exe` agent from argv.
//! The provider catalog shape, ACP launch metadata, and explicit unknown
//! authentication state are also Devboule code.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::process::{Command, Output};

#[cfg(windows)]
const DEFAULT_PATHEXT: &str = ".COM;.EXE;.BAT;.CMD";

/// A known CLI name and its aliases, with the ACP invocation when supported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownAgent {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub acp_args: Option<&'static [&'static str]>,
    /// Native stream-json argv, used only by the Claude adapter. Other
    /// agents stay on ACP; this field is None for them.
    pub stream_json_args: Option<&'static [&'static str]>,
}

/// Agents currently relevant to the first provider catalog slice.
///
/// Keep one row per agent so adding a known CLI does not require changing the
/// discovery algorithm.
/// Measured CLI 2.1.260 stream-json host-permission launch. Empty
/// `--setting-sources` is a literal empty argument, not an omitted flag.
pub const CLAUDE_STREAM_JSON_ARGS: &[&str] = &[
    "-p",
    "--output-format",
    "stream-json",
    "--input-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--strict-mcp-config",
    "--setting-sources",
    "",
    "--permission-prompts",
    "host",
    "--permission-prompt-tool",
    "stdio",
];

pub const KNOWN_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        id: "claude",
        aliases: &["claude", "claude-code"],
        acp_args: None,
        stream_json_args: Some(CLAUDE_STREAM_JSON_ARGS),
    },
    KnownAgent {
        id: "codex",
        aliases: &["codex"],
        acp_args: None,
        stream_json_args: None,
    },
    KnownAgent {
        id: "grok",
        aliases: &["grok", "grok-build"],
        acp_args: Some(&["agent", "stdio"]),
        stream_json_args: None,
    },
    KnownAgent {
        id: "pi",
        aliases: &["pi"],
        acp_args: None,
        stream_json_args: None,
    },
    KnownAgent {
        id: "qwen",
        aliases: &["qwen", "qwen-code", "qwen code"],
        // Measured 2026-09-04: `--acp` answers `initialize` and
        // `--experimental-acp` still works but prints "deprecated and will be
        // removed in a future release. Please use --acp instead."
        acp_args: Some(&["--acp"]),
        stream_json_args: None,
    },
    KnownAgent {
        id: "gemini",
        aliases: &["gemini"],
        acp_args: Some(&["--acp"]),
        stream_json_args: None,
    },
];

// Test-only provider: the integration stub binary. It resolves only when its
// build directory is on PATH, so end-user machines never list it; release
// builds drop the row entirely so a shipped daemon cannot discover it. The
// row exists so provider health measured against the stub (spawn + handshake
// outcomes) is visible through ProvidersList in integration tests, mirroring
// how real providers are surfaced.
#[cfg(debug_assertions)]
const TEST_ONLY_AGENTS: &[KnownAgent] = &[KnownAgent {
    id: "devboule-acp-stub",
    aliases: &["devboule-acp-stub"],
    acp_args: None,
    stream_json_args: None,
}];
#[cfg(not(debug_assertions))]
const TEST_ONLY_AGENTS: &[KnownAgent] = &[];

/// Registry wrappers that a better native chat-capable provider covers in the
/// workspace picker. This is an explicit product-policy map from §1.1:
/// `claude-acp` is a proprietary npx wrapper, while native `claude` already
/// speaks stream-json and reuses the user's Claude subscription. It stays in
/// Settings so the installed option remains honest and discoverable.
/// `codex-acp` is intentionally not covered because native codex is not
/// chat-capable; `pi-acp` is likewise not covered because native pi is not
/// chat-capable.
#[cfg(feature = "server")]
const REGISTRY_NATIVE_CHAT_COVERAGE: &[(&str, &str)] = &[("claude-acp", "claude")];

/// Explicit product policy for selecting the default ACP provider.
///
/// This is deliberately separate from `KNOWN_AGENTS` layout: on 2026-09-04,
/// grok was the only ACP agent verified to complete a working session on this
/// machine; qwen completed the handshake but declares its ACP flag deprecated.
const ACP_PREFERENCE: &[&str] = &["grok", "qwen", "gemini"];

/// Authentication is intentionally not probed by the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationStatus {
    Unknown,
}

/// How a catalog row was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderOrigin {
    UserBinary,
    NpxWrapper,
}

impl ProviderOrigin {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::UserBinary => "user-binary",
            Self::NpxWrapper => "npx-wrapper",
        }
    }
}

/// Direct CreateProcess program plus arguments that must precede ACP/user args.
///
/// A native CLI (`claude.exe`, `grok.exe`) has an empty `prefix_args`. An
/// unwrapped npm cmd-shim is `node` plus the package script path.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ResolvedLaunch {
    program: PathBuf,
    prefix_args: Vec<String>,
}

impl ResolvedLaunch {
    fn program(program: PathBuf) -> Self {
        Self {
            program,
            prefix_args: Vec::new(),
        }
    }
}

/// One known provider found on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledAgent {
    pub id: String,
    pub aliases: &'static [&'static str],
    pub executable: PathBuf,
    /// Arguments inserted between `executable` and ACP/user args.
    /// Empty for a native binary; the package script for an npm cmd-shim.
    pub prefix_args: Vec<String>,
    /// Resolved spawn argv for ACP: executable, then prefix args, then ACP
    /// flags. Element 0 is the path that CreateProcess will run.
    pub acp_command: Option<Vec<String>>,
    /// Resolved spawn argv for Claude stream-json: executable, then prefix
    /// args, then the measured flags. Element 0 is the path that CreateProcess
    /// will run.
    pub stream_json_command: Option<Vec<String>>,
    pub authentication: AuthenticationStatus,
    pub origin: ProviderOrigin,
    /// Registry-supplied arguments appended after `npx -y <package>`. None
    /// for native providers; Some, including an empty vector, for wrappers.
    pub launch_args: Option<Vec<String>>,
    /// Explicit picker policy. Covered wrappers are kept in Settings but are
    /// omitted from the workspace provider picker.
    pub pickable: Option<bool>,
}

/// PATH scan result. `unreadable_dirs` is the number of unique PATH entries
/// that could not be listed (I/O error, not "the directory is missing").
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDiscovery {
    pub agents: Vec<InstalledAgent>,
    pub unreadable_dirs: u32,
}

/// Return the known agents whose executable can be resolved from PATH.
pub fn discover() -> ProviderDiscovery {
    let directories = match std::env::var_os("PATH") {
        Some(paths) => std::env::split_paths(&paths).collect(),
        None => Vec::new(),
    };
    discover_in_paths(&directories)
}

pub(crate) fn discover_in_paths(directories: &[PathBuf]) -> ProviderDiscovery {
    let agents = KNOWN_AGENTS
        .iter()
        // Chained here (the only KNOWN_AGENTS consumption in this module) so
        // every discovery path — ProvidersList, find_in_catalog,
        // find_available — sees the test-only row in debug builds.
        .chain(TEST_ONLY_AGENTS.iter())
        .filter_map(|spec| {
            let launch = spec
                .aliases
                .iter()
                .find_map(|alias| resolve_launch_command_in_paths(directories, alias))?;
            let acp_command = spec
                .acp_args
                .map(|args| protocol_argv(&launch.program, &launch.prefix_args, args));
            let stream_json_command = spec
                .stream_json_args
                .map(|args| protocol_argv(&launch.program, &launch.prefix_args, args));
            Some(InstalledAgent {
                id: spec.id.to_string(),
                aliases: spec.aliases,
                executable: launch.program,
                prefix_args: launch.prefix_args,
                acp_command,
                stream_json_command,
                authentication: AuthenticationStatus::Unknown,
                origin: ProviderOrigin::UserBinary,
                launch_args: None,
                pickable: None,
            })
        })
        .collect();
    ProviderDiscovery {
        agents,
        unreadable_dirs: count_unreadable_dirs(directories),
    }
}

fn protocol_argv(executable: &Path, prefix_args: &[String], extra: &[&str]) -> Vec<String> {
    let mut command = Vec::with_capacity(1 + prefix_args.len() + extra.len());
    command.push(executable.to_string_lossy().into_owned());
    command.extend(prefix_args.iter().cloned());
    command.extend(extra.iter().map(|arg| (*arg).to_string()));
    command
}

fn count_unreadable_dirs(directories: &[PathBuf]) -> u32 {
    let mut seen = HashSet::new();
    let mut count = 0;
    for dir in directories {
        if dir.as_os_str().is_empty() || !seen.insert(dir.clone()) {
            continue;
        }
        match std::fs::read_dir(dir) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => count += 1,
        }
    }
    count
}

/// Chat dialect this installed agent can speak, if any.
/// An agent with both launches is offered as ACP: ACP is the road, stream-json the exception.
pub fn chat_protocol(agent: &InstalledAgent) -> Option<&'static str> {
    if agent.acp_command.is_some() {
        Some("acp")
    } else if agent.stream_json_command.is_some() {
        Some("stream-json")
    } else {
        None
    }
}

/// Return the first discovered provider that offers ACP.
pub fn first_acp_available() -> Option<InstalledAgent> {
    let discovered = discover();
    ACP_PREFERENCE.iter().find_map(|preferred_id| {
        discovered
            .agents
            .iter()
            .find(|agent| agent.id == *preferred_id && agent.acp_command.is_some())
            .cloned()
    })
}

/// Return the discovered provider with this catalog id, if it is on PATH.
pub fn find_available(id: &str) -> Option<InstalledAgent> {
    discover().agents.into_iter().find(|agent| agent.id == id)
}

#[cfg(feature = "server")]
fn path_directories() -> Vec<PathBuf> {
    match std::env::var_os("PATH") {
        Some(paths) => std::env::split_paths(&paths).collect(),
        None => Vec::new(),
    }
}

/// Local PATH scan plus ACP-registry npx rows. Native ids/aliases win.
#[cfg(feature = "server")]
pub fn discover_catalog(
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
) -> ProviderDiscovery {
    discover_catalog_in_paths(fetch, cache_dir, &path_directories())
}

#[cfg(feature = "server")]
pub(crate) fn discover_catalog_in_paths(
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
    directories: &[PathBuf],
) -> ProviderDiscovery {
    let local = discover_in_paths(directories);
    let registry = crate::registry::load_npx_entries(fetch, cache_dir)
        .into_iter()
        .map(|entry| registry_agent(directories, &local, entry))
        .collect();
    merge_native_beats_registry(local, registry)
}

#[cfg(feature = "server")]
fn registry_agent(
    directories: &[PathBuf],
    native: &ProviderDiscovery,
    entry: crate::registry::RegistryNpxEntry,
) -> InstalledAgent {
    let crate::registry::RegistryNpxEntry { id, package, args } = entry;
    let pickable = registry_picker_policy(&id, native);
    let acp_command = npx_acp_command(directories, &package, &args);
    InstalledAgent {
        id,
        aliases: &[],
        executable: PathBuf::from(&package),
        prefix_args: Vec::new(),
        acp_command,
        stream_json_command: None,
        authentication: AuthenticationStatus::Unknown,
        origin: ProviderOrigin::NpxWrapper,
        launch_args: Some(args),
        pickable,
    }
}

#[cfg(feature = "server")]
fn registry_picker_policy(id: &str, native: &ProviderDiscovery) -> Option<bool> {
    let covering_native = REGISTRY_NATIVE_CHAT_COVERAGE
        .iter()
        .find(|(wrapper, _)| id.eq_ignore_ascii_case(wrapper))
        .map(|(_, native_id)| *native_id)?;
    native
        .agents
        .iter()
        .any(|agent| {
            agent.origin == ProviderOrigin::UserBinary
                && agent.id.eq_ignore_ascii_case(covering_native)
                && chat_protocol(agent).is_some()
        })
        .then_some(false)
}

#[cfg(feature = "server")]
fn launch_program_is_cmd_or_bat(program: &Path) -> bool {
    program
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat"))
}

#[cfg(feature = "server")]
fn npx_acp_command(directories: &[PathBuf], package: &str, args: &[String]) -> Option<Vec<String>> {
    let launch = resolve_launch_command_in_paths(directories, "npx")?;
    if launch_program_is_cmd_or_bat(&launch.program) {
        return None;
    }
    let mut extra = Vec::with_capacity(2 + args.len());
    extra.push("-y".to_string());
    extra.push(package.to_string());
    extra.extend(args.iter().cloned());
    let extra_refs: Vec<&str> = extra.iter().map(String::as_str).collect();
    Some(protocol_argv(
        &launch.program,
        &launch.prefix_args,
        &extra_refs,
    ))
}

#[cfg(feature = "server")]
fn merge_native_beats_registry(
    mut local: ProviderDiscovery,
    registry: Vec<InstalledAgent>,
) -> ProviderDiscovery {
    let occupied: HashSet<String> = local
        .agents
        .iter()
        .flat_map(|agent| {
            std::iter::once(agent.id.clone())
                .chain(agent.aliases.iter().map(|alias| (*alias).to_string()))
        })
        .map(|name| name.to_ascii_lowercase())
        .collect();
    for row in registry {
        if occupied.contains(&row.id.to_ascii_lowercase()) {
            continue;
        }
        local.agents.push(row);
    }
    local
}

/// PATH scan plus registry, then the named row if it exists.
#[cfg(feature = "server")]
pub fn find_in_catalog(
    id: &str,
    fetch: &dyn crate::registry::RegistryFetch,
    cache_dir: &Path,
) -> Option<InstalledAgent> {
    discover_catalog(fetch, cache_dir)
        .agents
        .into_iter()
        .find(|agent| agent.id == id)
}

pub(crate) fn executable_file_exists(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    {
        true
    }
}

fn launch_path_candidates(dir: &Path, command: &str) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        launch_path_candidates_for_pathext(dir, command, std::env::var("PATHEXT").ok().as_deref())
    }

    #[cfg(not(windows))]
    {
        launch_path_candidates_for_pathext(dir, command, None)
    }
}

fn launch_path_candidates_for_pathext(
    dir: &Path,
    command: &str,
    pathext: Option<&str>,
) -> Vec<PathBuf> {
    let base = dir.join(command);

    #[cfg(not(windows))]
    {
        let _ = pathext;
        vec![base]
    }

    #[cfg(windows)]
    {
        if let Some(extension) = Path::new(command).extension().and_then(|ext| ext.to_str()) {
            return is_direct_launch_extension(&format!(".{extension}"))
                .then_some(base)
                .into_iter()
                .collect();
        }

        let pathext = pathext
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(DEFAULT_PATHEXT);
        pathext
            .split(';')
            .map(str::trim)
            .filter(|extension| !extension.is_empty())
            .filter_map(|extension| {
                let extension = if extension.starts_with('.') {
                    extension.to_string()
                } else {
                    format!(".{extension}")
                };
                is_direct_launch_extension(&extension)
                    .then(|| dir.join(format!("{command}{extension}")))
            })
            .collect()
    }
}

#[cfg(windows)]
// PATHEXT also names interpreter scripts such as PS1, VBS, and JS. They are
// intentionally excluded: spawn_process uses Command/CreateProcess directly,
// without an interpreter, so a PS1-only installation is not launchable.
fn is_direct_launch_extension(extension: &str) -> bool {
    [".COM", ".EXE", ".BAT", ".CMD"]
        .iter()
        .any(|known| known.eq_ignore_ascii_case(extension))
}

fn resolve_direct_program(paths: &[PathBuf], command: &str) -> Option<PathBuf> {
    paths.iter().find_map(|dir| {
        launch_path_candidates(dir, command)
            .into_iter()
            .find_map(|path| executable_file_exists(&path).then(|| absolute_path(&path)))
            .flatten()
    })
}

fn resolve_launch_command_in_paths(paths: &[PathBuf], command: &str) -> Option<ResolvedLaunch> {
    let program = resolve_direct_program(paths, command)?;
    #[cfg(windows)]
    if let Some(unwrapped) = unwrap_windows_npm_cmd_shim(&program, paths) {
        return Some(unwrapped);
    }
    Some(ResolvedLaunch::program(program))
}

/// npm's `cmd-shim` writes a batch that assigns `_prog` to `node` (or a
/// sibling `node.exe`) and then launches `"%_prog%" "%dp0%\<script>" %*`.
/// Measured on this machine for `codex.cmd`, `pi.cmd`, and `qwen.cmd`.
fn is_npm_node_cmd_shim(contents: &str) -> bool {
    let lower = contents.to_ascii_lowercase();
    lower.contains(r#"set "_prog=node""#) || lower.contains(r#"set "_prog=%dp0%\node.exe""#)
}

/// Relative script path from an npm cmd-shim, if the launch line is the
/// measured `"%_prog%" "%dp0%\<script>" %*` form. Anything else is left alone.
fn npm_cmd_shim_script_relative(contents: &str) -> Option<&str> {
    if !is_npm_node_cmd_shim(contents) {
        return None;
    }

    let mut rest = contents;
    while let Some(offset) = rest.find(r#""%_prog%""#) {
        rest = &rest[offset + r#""%_prog%""#.len()..];
        let trimmed = rest.trim_start_matches([' ', '\t']);
        let Some((relative, after)) = split_quoted_dp0_path(trimmed) else {
            continue;
        };
        let after = after.trim_start_matches([' ', '\t']);
        if after.starts_with("%*") && !relative.is_empty() {
            return Some(relative);
        }
    }
    None
}

fn split_quoted_dp0_path(input: &str) -> Option<(&str, &str)> {
    let rest = input.strip_prefix('"')?;
    let rest = rest
        .strip_prefix("%dp0%")
        .or_else(|| rest.strip_prefix("%~dp0"))?;
    let rest = rest.trim_start_matches(['\\', '/']);
    let end = rest.find('"')?;
    Some((&rest[..end], &rest[end + 1..]))
}

/// npm's own launcher shim (npx.cmd, npm.cmd) uses a different batch shape
/// than per-package cmd-shims. It assigns a node executable variable and a
/// script variable from `%~dp0`, then invokes
/// `"%NODE_VAR%" "%SCRIPT_VAR%" %*`. Measured on this machine for npx.cmd
/// (npm 10.x).
///
/// Recognized by the `SET "NODE_EXE=%~dp0\node.exe"` (or `%dp0%`) line.
fn is_npm_launcher_shim(contents: &str) -> bool {
    let lower = contents.to_ascii_lowercase();
    lower.contains(r#"set "node_exe=%~dp0\node.exe""#)
        || lower.contains(r#"set "node_exe=%dp0%\node.exe""#)
        || lower.contains(r#"set "node_exe=%~dp0/node.exe""#)
        || lower.contains(r#"set "node_exe=%dp0%/node.exe""#)
}

/// Relative script path from an npm launcher shim, if recognized.
///
/// Parses `SET "VAR=%~dp0\..."` assignments to find the node executable
/// variable (value ends in `node.exe`) and the script variable (value ends in
/// `.js`). Then looks for the final invocation line
/// `"%NODE_VAR%" "%SCRIPT_VAR%" %*` and returns the relative path from the
/// last static `%~dp0` / `%dp0%` assignment for the script variable.
///
/// The FOR /F dynamic override (if present) is intentionally ignored — the
/// static `%~dp0\node_modules\npm\bin\npx-cli.js` always exists in a real npm
/// install and is the correct fallback.
fn npm_launcher_shim_script_relative(contents: &str) -> Option<&str> {
    if !is_npm_launcher_shim(contents) {
        return None;
    }

    let mut node_var_name: Option<&str> = None;
    let mut script_var_name: Option<&str> = None;
    let mut script_relative: Option<&str> = None;

    for line in contents.lines() {
        let trimmed = line.trim();
        // Match: SET "VAR=%~dp0\path" or SET "VAR=%dp0%\path". A non-SET line
        // (@ECHO, IF, FOR, the final invocation) is skipped, not a parse
        // failure — the launcher body is mostly non-SET lines.
        let Some(rest) = trimmed
            .strip_prefix("SET \"")
            .or_else(|| trimmed.strip_prefix("set \""))
        else {
            continue;
        };
        let Some(eq) = rest.find('=') else {
            continue;
        };
        let var_name = &rest[..eq];
        let value = &rest[eq + 1..];
        let value = value.strip_suffix('"').unwrap_or(value);

        // Not a dp0-relative assignment (e.g. SET "NODE_EXE=node") — skip.
        let Some(relative) = value
            .strip_prefix("%~dp0\\")
            .or_else(|| value.strip_prefix("%dp0%\\"))
            .or_else(|| value.strip_prefix("%~dp0/"))
            .or_else(|| value.strip_prefix("%dp0%/"))
        else {
            continue;
        };

        let value_lower = value.to_ascii_lowercase();
        if value_lower.ends_with("node.exe") {
            node_var_name = Some(var_name);
        } else if value_lower.ends_with(".js") {
            script_var_name = Some(var_name);
            script_relative = Some(relative);
        }
    }

    let node_var = node_var_name?;
    let script_var = script_var_name?;
    let relative = script_relative?;

    // Find the final invocation line: "%NODE_VAR%" "%SCRIPT_VAR%" %*
    let node_pat = format!("\"%{node_var}%\"");
    let script_pat = format!("\"%{script_var}%\"");

    let mut rest = contents;
    while let Some(offset) = rest.find(&node_pat) {
        let after_node = &rest[offset + node_pat.len()..];
        let after_node = after_node.trim_start_matches([' ', '\t']);
        let Some(after_script) = after_node.strip_prefix(&script_pat) else {
            rest = &rest[offset + node_pat.len()..];
            continue;
        };
        let after_script = after_script.trim_start_matches([' ', '\t']);
        if after_script.starts_with("%*") && !relative.is_empty() {
            return Some(relative);
        }
        rest = &rest[offset + node_pat.len()..];
    }
    None
}

#[cfg(windows)]
fn unwrap_windows_npm_cmd_shim(shim: &Path, search_paths: &[PathBuf]) -> Option<ResolvedLaunch> {
    let extension = shim.extension()?.to_str()?;
    if !extension.eq_ignore_ascii_case("cmd") && !extension.eq_ignore_ascii_case("bat") {
        return None;
    }

    let contents = std::fs::read_to_string(shim).ok()?;

    // Try per-package cmd-shim shape first ("%_prog%" "%dp0%\<script>" %*).
    if let Some(relative) = npm_cmd_shim_script_relative(&contents) {
        return resolve_shim_script(shim, search_paths, relative);
    }

    // Fall back to npm launcher shape ("%NODE_EXE%" "%NPX_CLI_JS%" %*).
    if let Some(relative) = npm_launcher_shim_script_relative(&contents) {
        return resolve_shim_script(shim, search_paths, relative);
    }

    None
}

/// Shared resolution logic: build the script path from the shim directory +
/// relative path, check `..` traversal and existence, resolve `node.exe` from
/// the shim sibling or PATH, and return a `ResolvedLaunch`.
#[cfg(windows)]
fn resolve_shim_script(
    shim: &Path,
    search_paths: &[PathBuf],
    relative: &str,
) -> Option<ResolvedLaunch> {
    let shim_dir = shim.parent()?;
    let mut script = shim_dir.to_path_buf();
    for component in relative.split(['/', '\\']) {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            return None;
        }
        script.push(component);
    }
    if !script.is_file() {
        return None;
    }

    let node = {
        let local_node = shim_dir.join("node.exe");
        if executable_file_exists(&local_node) {
            absolute_path(&local_node)
        } else {
            resolve_direct_program(search_paths, "node")
        }
    }?;
    let script = absolute_path(&script)?;
    Some(ResolvedLaunch {
        program: node,
        prefix_args: vec![script.to_string_lossy().into_owned()],
    })
}

fn absolute_path(path: &Path) -> Option<PathBuf> {
    let absolute = std::fs::canonicalize(path).ok().or_else(|| {
        if path.is_absolute() {
            Some(path.to_path_buf())
        } else {
            std::env::current_dir().ok().map(|cwd| cwd.join(path))
        }
    })?;

    #[cfg(windows)]
    {
        Some(normalize_windows_path(absolute))
    }

    #[cfg(not(windows))]
    {
        Some(absolute)
    }
}

#[cfg(windows)]
fn normalize_windows_path(path: PathBuf) -> PathBuf {
    use std::ffi::{OsStr, OsString};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    let strip_prefix = |prefix: &str| {
        let path = path.as_os_str().encode_wide().collect::<Vec<_>>();
        let prefix = OsStr::new(prefix).encode_wide().collect::<Vec<_>>();
        path.starts_with(&prefix)
            .then(|| OsString::from_wide(&path[prefix.len()..]))
    };

    if let Some(rest) = strip_prefix(r"\\?\UNC\") {
        let mut normalized = OsString::from_wide(&[b'\\' as u16, b'\\' as u16]);
        normalized.push(rest);
        return PathBuf::from(normalized);
    }

    if let Some(rest) = strip_prefix(r"\\?\") {
        let rest = PathBuf::from(rest);
        if rest.is_absolute() {
            return rest;
        }
    }

    path
}

#[cfg(test)]
fn probe_version(agent: &InstalledAgent) -> std::io::Result<Output> {
    Command::new(&agent.executable)
        .args(&agent.prefix_args)
        .arg("--version")
        .output()
}

#[cfg(test)]
mod tests {
    use super::discover;
    #[cfg(windows)]
    use super::{
        executable_file_exists, launch_path_candidates_for_pathext,
        resolve_launch_command_in_paths, ResolvedLaunch,
    };
    #[cfg(windows)]
    use std::fs::{self, File};
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "devboule-provider-catalog-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn command_name_candidate_shape_is_platform_specific() {
        let dir = PathBuf::from("provider-catalog-test");
        let candidates =
            super::launch_path_candidates_for_pathext(&dir, "agent", Some(".EXE;.CMD"));

        #[cfg(windows)]
        assert_eq!(
            candidates,
            vec![dir.join("agent.EXE"), dir.join("agent.CMD")]
        );
        #[cfg(not(windows))]
        assert_eq!(candidates, vec![dir.join("agent")]);
    }

    #[cfg(windows)]
    #[test]
    fn windows_fake_npm_shims_resolve_cmd_and_ps1() {
        let dir = temporary_directory("npm-shims");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("codex")).expect("fake POSIX shim");
        File::create(dir.join("codex.cmd")).expect("fake cmd shim");
        File::create(dir.join("qwen")).expect("fake POSIX shim");
        File::create(dir.join("qwen.ps1")).expect("fake powershell shim");
        File::create(dir.join("qwen.cmd")).expect("fake cmd shim");

        let paths = vec![dir.clone()];
        assert_eq!(
            resolve_launch_command_in_paths(&paths, "codex"),
            Some(ResolvedLaunch::program(super::normalize_windows_path(
                fs::canonicalize(dir.join("codex.cmd")).expect("canonical cmd shim"),
            )))
        );
        assert_eq!(
            resolve_launch_command_in_paths(&paths, "qwen"),
            Some(ResolvedLaunch::program(super::normalize_windows_path(
                fs::canonicalize(dir.join("qwen.cmd")).expect("canonical cmd shim"),
            )))
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    fn npm_cmd_shim_contents(script_from_dp0: &str) -> String {
        format!(
            "\
@ECHO off
GOTO start
:find_dp0
SET dp0=%~dp0
EXIT /b
:start
SETLOCAL
CALL :find_dp0

IF EXIST \"%dp0%\\node.exe\" (
  SET \"_prog=%dp0%\\node.exe\"
) ELSE (
  SET \"_prog=node\"
  SET PATHEXT=%PATHEXT:;.JS;=;%
)

endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\{script_from_dp0}\" %*
"
        )
    }

    #[cfg(windows)]
    fn canonical(path: &Path) -> PathBuf {
        super::normalize_windows_path(fs::canonicalize(path).expect("canonical path"))
    }

    #[cfg(windows)]
    #[test]
    fn windows_npm_cmd_shim_resolves_to_node_and_package_script() {
        let dir = temporary_directory("npm-real-shim");
        let node_dir = temporary_directory("npm-real-shim-node");
        fs::create_dir_all(
            dir.join("node_modules")
                .join("@openai")
                .join("codex")
                .join("bin"),
        )
        .expect("package directory");
        fs::create_dir_all(&node_dir).expect("node directory");
        File::create(dir.join("codex")).expect("fake POSIX shim");
        fs::write(
            dir.join("codex.cmd"),
            npm_cmd_shim_contents(r"node_modules\@openai\codex\bin\codex.js"),
        )
        .expect("npm cmd shim");
        fs::write(
            dir.join("node_modules")
                .join("@openai")
                .join("codex")
                .join("bin")
                .join("codex.js"),
            "console.log('codex');\n",
        )
        .expect("package script");
        File::create(node_dir.join("node.exe")).expect("fake node");

        let paths = vec![dir.clone(), node_dir.clone()];
        let resolved = resolve_launch_command_in_paths(&paths, "codex");
        assert_eq!(
            resolved,
            Some(ResolvedLaunch {
                program: canonical(&node_dir.join("node.exe")),
                prefix_args: vec![canonical(
                    &dir.join("node_modules")
                        .join("@openai")
                        .join("codex")
                        .join("bin")
                        .join("codex.js"),
                )
                .to_string_lossy()
                .into_owned()],
            })
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
        fs::remove_dir_all(node_dir).expect("temporary node directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_npm_cmd_shim_prefers_sibling_node_exe() {
        let dir = temporary_directory("npm-local-node");
        fs::create_dir_all(dir.join("node_modules").join("pkg")).expect("package directory");
        File::create(dir.join("codex")).expect("fake POSIX shim");
        fs::write(
            dir.join("codex.cmd"),
            npm_cmd_shim_contents(r"node_modules\pkg\cli.js"),
        )
        .expect("npm cmd shim");
        fs::write(
            dir.join("node_modules").join("pkg").join("cli.js"),
            "/* js */\n",
        )
        .expect("package script");
        File::create(dir.join("node.exe")).expect("sibling node");

        let resolved = resolve_launch_command_in_paths(std::slice::from_ref(&dir), "codex");
        assert_eq!(
            resolved,
            Some(ResolvedLaunch {
                program: canonical(&dir.join("node.exe")),
                prefix_args: vec![
                    canonical(&dir.join("node_modules").join("pkg").join("cli.js"))
                        .to_string_lossy()
                        .into_owned()
                ],
            })
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_npm_cmd_shim_without_script_stays_the_cmd() {
        let dir = temporary_directory("npm-missing-script");
        let node_dir = temporary_directory("npm-missing-script-node");
        fs::create_dir_all(&dir).expect("temporary directory");
        fs::create_dir_all(&node_dir).expect("node directory");
        fs::write(
            dir.join("codex.cmd"),
            npm_cmd_shim_contents(r"node_modules\@openai\codex\bin\codex.js"),
        )
        .expect("npm cmd shim");
        File::create(node_dir.join("node.exe")).expect("fake node");

        assert_eq!(
            resolve_launch_command_in_paths(&[dir.clone(), node_dir.clone()], "codex"),
            Some(ResolvedLaunch::program(canonical(&dir.join("codex.cmd"))))
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
        fs::remove_dir_all(node_dir).expect("temporary node directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_npm_cmd_shim_without_node_stays_the_cmd() {
        let dir = temporary_directory("npm-missing-node");
        fs::create_dir_all(dir.join("node_modules").join("pkg")).expect("package directory");
        fs::write(
            dir.join("codex.cmd"),
            npm_cmd_shim_contents(r"node_modules\pkg\cli.js"),
        )
        .expect("npm cmd shim");
        fs::write(
            dir.join("node_modules").join("pkg").join("cli.js"),
            "/* js */\n",
        )
        .expect("package script");

        assert_eq!(
            resolve_launch_command_in_paths(std::slice::from_ref(&dir), "codex"),
            Some(ResolvedLaunch::program(canonical(&dir.join("codex.cmd"))))
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_native_exe_is_not_unwrapped() {
        let dir = temporary_directory("native-exe");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("grok.exe")).expect("native grok");

        assert_eq!(
            resolve_launch_command_in_paths(std::slice::from_ref(&dir), "grok"),
            Some(ResolvedLaunch::program(canonical(&dir.join("grok.exe"))))
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[test]
    fn npm_cmd_shim_parser_reads_measured_launch_lines() {
        let cases = [
            (
                r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\@openai\codex\bin\codex.js" %*"#,
                r"node_modules\@openai\codex\bin\codex.js",
            ),
            (
                r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\@earendil-works\pi-coding-agent\dist\bundle\cli.js" %*"#,
                r"node_modules\@earendil-works\pi-coding-agent\dist\bundle\cli.js",
            ),
            (
                r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\@qwen-code\qwen-code\cli-entry.js" %*"#,
                r"node_modules\@qwen-code\qwen-code\cli-entry.js",
            ),
            (
                r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\pnpm\bin\pnpm.cjs" %*"#,
                r"node_modules\pnpm\bin\pnpm.cjs",
            ),
        ];
        for (contents, expected) in cases {
            assert_eq!(
                super::npm_cmd_shim_script_relative(contents),
                Some(expected),
                "{contents}"
            );
        }
        assert_eq!(
            super::npm_cmd_shim_script_relative("@ECHO off\ncodex --version\n"),
            None
        );
    }

    #[test]
    fn npm_launcher_shim_relative_extracts_script_from_real_npx_cmd() {
        // Verbatim tail of the real npx.cmd on this machine (npm 10.x).
        let contents = "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
IF NOT EXIST \"%NODE_EXE%\" ( SET \"NODE_EXE=node\" )\n\
SET \"NPM_PREFIX_JS=%~dp0\\node_modules\\npm\\bin\\npm-prefix.js\"\n\
SET \"NPX_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npx-cli.js\"\n\
FOR /F \"delims=\" %%F IN ('CALL \"%NODE_EXE%\" \"%NPM_PREFIX_JS%\"') DO ( SET \"NPM_PREFIX_NPX_CLI_JS=%%F\\node_modules\\npm\\bin\\npx-cli.js\" )\n\
IF EXIST \"%NPM_PREFIX_NPX_CLI_JS%\" ( SET \"NPX_CLI_JS=%NPM_PREFIX_NPX_CLI_JS%\" )\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n";
        assert_eq!(
            super::npm_launcher_shim_script_relative(contents),
            Some(r"node_modules\npm\bin\npx-cli.js")
        );
    }

    #[test]
    fn npm_launcher_shim_relative_handles_forward_slashes_in_set() {
        let contents = "\
SET \"NODE_EXE=%~dp0/node.exe\"\n\
SET \"CLI_JS=%~dp0/node_modules/pkg/cli.js\"\n\
\"%NODE_EXE%\" \"%CLI_JS%\" %*\n";
        assert_eq!(
            super::npm_launcher_shim_script_relative(contents),
            Some("node_modules/pkg/cli.js")
        );
    }

    #[test]
    fn npm_launcher_shim_relative_returns_none_when_node_exe_is_missing() {
        let contents = "\
SET \"CLI_JS=%~dp0\\cli.js\"\n\
\"%NODE_EXE%\" \"%CLI_JS%\" %*\n";
        assert_eq!(super::npm_launcher_shim_script_relative(contents), None);
    }

    #[test]
    fn npm_launcher_shim_relative_returns_none_for_non_launcher_contents() {
        assert_eq!(
            super::npm_launcher_shim_script_relative("@ECHO off\necho hello\n"),
            None
        );
        // Per-package cmd-shim shape should NOT match the launcher parser.
        assert_eq!(
            super::npm_launcher_shim_script_relative(
                r#"SET "_prog=node"
"%_prog%"  "%dp0%\node_modules\pkg\cli.js" %*"#
            ),
            None
        );
    }

    #[cfg(windows)]
    #[test]
    fn npm_launcher_shim_rejects_dot_dot_traversal() {
        let dir = temporary_directory("launcher-dotdot");
        fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
        File::create(dir.join("node.exe")).expect("node");
        fs::write(
            dir.join("npx.cmd"),
            "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
SET \"NPX_CLI_JS=%~dp0\\..\\escape\\npx-cli.js\"\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n",
        )
        .expect("npx.cmd");
        fs::create_dir_all(dir.join("..").join("escape")).expect("escape dir");
        fs::write(
            dir.join("..").join("escape").join("npx-cli.js"),
            "/* npx */\n",
        )
        .expect("escape script");
        // The script exists but the relative path contains .. — must be None.
        assert_eq!(
            super::resolve_launch_command_in_paths(std::slice::from_ref(&dir), "npx"),
            Some(ResolvedLaunch::program(canonical(&dir.join("npx.cmd"))))
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_npm_launcher_shim_unwraps_to_node_and_script() {
        let dir = temporary_directory("launcher-unwrap");
        fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
        File::create(dir.join("node.exe")).expect("node");
        // Real npx.cmd launcher shape.
        fs::write(
            dir.join("npx.cmd"),
            "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
IF NOT EXIST \"%NODE_EXE%\" ( SET \"NODE_EXE=node\" )\n\
SET \"NPM_PREFIX_JS=%~dp0\\node_modules\\npm\\bin\\npm-prefix.js\"\n\
SET \"NPX_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npx-cli.js\"\n\
FOR /F \"delims=\" %%F IN ('CALL \"%NODE_EXE%\" \"%NPM_PREFIX_JS%\"') DO ( SET \"NPM_PREFIX_NPX_CLI_JS=%%F\\node_modules\\npm\\bin\\npx-cli.js\" )\n\
IF EXIST \"%NPM_PREFIX_NPX_CLI_JS%\" ( SET \"NPX_CLI_JS=%NPM_PREFIX_NPX_CLI_JS%\" )\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n",
        )
        .expect("npx.cmd");
        fs::write(
            dir.join("node_modules")
                .join("npm")
                .join("bin")
                .join("npx-cli.js"),
            "/* npx */\n",
        )
        .expect("npx-cli.js");

        let resolved = resolve_launch_command_in_paths(std::slice::from_ref(&dir), "npx");
        assert_eq!(
            resolved,
            Some(ResolvedLaunch {
                program: canonical(&dir.join("node.exe")),
                prefix_args: vec![canonical(
                    &dir.join("node_modules")
                        .join("npm")
                        .join("bin")
                        .join("npx-cli.js"),
                )
                .to_string_lossy()
                .into_owned()],
            })
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_launch_candidates_follow_pathext_and_skip_the_naked_name() {
        let dir = temporary_directory("pathext");
        fs::create_dir_all(&dir).expect("temporary directory");

        assert_eq!(
            launch_path_candidates_for_pathext(&dir, "agent", Some(".BAT;.CMD;.EXE")),
            vec![
                dir.join("agent.BAT"),
                dir.join("agent.CMD"),
                dir.join("agent.EXE"),
            ]
        );
        assert_eq!(
            launch_path_candidates_for_pathext(&dir, "agent.cmd", Some(".BAT;.CMD;.EXE")),
            vec![dir.join("agent.cmd")]
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    fn existing_launch_candidates(dir: &Path, command: &str, pathext: &str) -> Vec<PathBuf> {
        launch_path_candidates_for_pathext(dir, command, Some(pathext))
            .into_iter()
            .filter(|path| executable_file_exists(path))
            .collect()
    }

    #[cfg(windows)]
    #[test]
    fn windows_only_powershell_shim_is_not_launchable() {
        let dir = temporary_directory("ps1-only");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("agent.ps1")).expect("fake powershell shim");

        assert!(existing_launch_candidates(&dir, "agent", ".PS1").is_empty());

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_cmd_wins_over_powershell_even_when_pathext_prefers_ps1() {
        let dir = temporary_directory("ps1-before-cmd");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("agent.ps1")).expect("fake powershell shim");
        File::create(dir.join("agent.cmd")).expect("fake cmd shim");

        assert_eq!(
            existing_launch_candidates(&dir, "agent", ".PS1;.CMD"),
            vec![dir.join("agent.CMD")]
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_ignores_pathext_entries_without_a_direct_launcher() {
        let dir = temporary_directory("unsupported-pathext");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("agent.VBS")).expect("fake visual basic script");
        File::create(dir.join("agent.JS")).expect("fake javascript script");

        assert!(existing_launch_candidates(&dir, "agent", ".VBS;.JS").is_empty());

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_pathext_fallback_handles_empty_space_and_malformed_entries() {
        let dir = temporary_directory("pathext-fallback");
        fs::create_dir_all(&dir).expect("temporary directory");

        let default_candidates = vec![
            dir.join("agent.COM"),
            dir.join("agent.EXE"),
            dir.join("agent.BAT"),
            dir.join("agent.CMD"),
        ];
        for pathext in [None, Some(""), Some("   "), Some(".VBS;.JS")] {
            let expected = if pathext == Some(".VBS;.JS") {
                Vec::new()
            } else {
                default_candidates.clone()
            };
            assert_eq!(
                launch_path_candidates_for_pathext(&dir, "agent", pathext),
                expected
            );
        }
        assert_eq!(
            launch_path_candidates_for_pathext(&dir, "agent", Some("eXe;cMd;.PS1")),
            vec![dir.join("agent.eXe"), dir.join("agent.cMd")]
        );

        fs::remove_dir_all(dir).expect("temporary directory cleanup");
    }

    #[cfg(windows)]
    #[test]
    fn windows_resolved_paths_drop_verbatim_prefix_without_breaking_unc() {
        assert_eq!(
            super::normalize_windows_path(PathBuf::from(r"\\?\C:\Users\Name\agent.exe")),
            PathBuf::from(r"C:\Users\Name\agent.exe")
        );
        assert_eq!(
            super::normalize_windows_path(PathBuf::from(r"\\?\UNC\server\share\agent.exe")),
            PathBuf::from(r"\\server\share\agent.exe")
        );
    }

    fn launch_display(agent: &super::InstalledAgent) -> String {
        let mut line = agent.executable.display().to_string();
        for arg in &agent.prefix_args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }

    fn fake_cli_path(dir: &Path, name: &str) {
        #[cfg(windows)]
        File::create(dir.join(format!("{name}.exe"))).expect("fake cli");
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = dir.join(name);
            std::fs::write(&path, []).expect("fake cli");
            let mut perms = std::fs::metadata(&path).expect("meta").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).expect("chmod");
        }
    }

    #[test]
    fn chat_protocol_reports_stream_json_acp_or_none() {
        let dir = temporary_directory("chat-protocol");
        std::fs::create_dir_all(&dir).expect("temporary directory");
        fake_cli_path(&dir, "claude");
        fake_cli_path(&dir, "grok");
        fake_cli_path(&dir, "codex");
        let discovered = super::discover_in_paths(std::slice::from_ref(&dir));
        let protocol_of = |id: &str| {
            discovered
                .agents
                .iter()
                .find(|agent| agent.id == id)
                .and_then(super::chat_protocol)
        };
        assert_eq!(protocol_of("claude"), Some("stream-json"));
        assert_eq!(protocol_of("grok"), Some("acp"));
        assert_eq!(protocol_of("codex"), None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn protocol_argv_inserts_prefix_between_executable_and_flags() {
        let exe = PathBuf::from(r"C:\nvm\node.exe");
        let prefix = vec![r"C:\npm\claude.js".to_string()];
        let argv = super::protocol_argv(&exe, &prefix, &["-p", "--verbose"]);
        assert_eq!(
            argv,
            vec![
                r"C:\nvm\node.exe".to_string(),
                r"C:\npm\claude.js".to_string(),
                "-p".to_string(),
                "--verbose".to_string(),
            ]
        );
    }

    #[test]
    fn protocol_command_element_zero_is_the_resolved_executable() {
        let dir = temporary_directory("honest-argv");
        #[cfg(windows)]
        {
            fs::create_dir_all(&dir).expect("temporary directory");
            File::create(dir.join("claude.exe")).expect("fake claude");
            File::create(dir.join("grok.exe")).expect("fake grok");
        }
        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(&dir).expect("temporary directory");
            std::fs::write(dir.join("claude"), []).expect("fake claude");
            std::fs::write(dir.join("grok"), []).expect("fake grok");
            for name in ["claude", "grok"] {
                let path = dir.join(name);
                let mut perms = std::fs::metadata(&path).expect("meta").permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&path, perms).expect("chmod");
            }
        }
        let discovered = super::discover_in_paths(std::slice::from_ref(&dir));
        let claude = discovered
            .agents
            .iter()
            .find(|agent| agent.id == "claude")
            .expect("claude on fake PATH");
        let stream = claude
            .stream_json_command
            .as_ref()
            .expect("claude speaks stream-json");
        assert_eq!(
            stream[0],
            claude.executable.to_string_lossy(),
            "stream_json_command[0] must be the resolved executable, not the catalog id"
        );
        let mut expected = vec![claude.executable.to_string_lossy().into_owned()];
        expected.extend(claude.prefix_args.iter().cloned());
        expected.extend(
            super::CLAUDE_STREAM_JSON_ARGS
                .iter()
                .map(|arg| (*arg).to_string()),
        );
        assert_eq!(
            stream, &expected,
            "spawned stream-json argv must stay executable + prefix + measured flags"
        );

        let grok = discovered
            .agents
            .iter()
            .find(|agent| agent.id == "grok")
            .expect("grok on fake PATH");
        let acp = grok.acp_command.as_ref().expect("grok speaks ACP");
        assert_eq!(
            acp[0],
            grok.executable.to_string_lossy(),
            "acp_command[0] must be the resolved executable, not the catalog id"
        );
        let mut expected = vec![grok.executable.to_string_lossy().into_owned()];
        expected.extend(grok.prefix_args.iter().cloned());
        expected.extend(["agent", "stdio"].iter().map(|arg| (*arg).to_string()));
        assert_eq!(
            acp, &expected,
            "spawned ACP argv must stay executable + prefix + acp args"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    #[ignore = "measurement, not an assertion; run by hand with --ignored --nocapture"]
    fn reports_installed_cli_agents() {
        let agents = discover().agents;
        println!("provider catalog found {} agent(s):", agents.len());
        for agent in agents {
            println!(
                "{} => {} | ACP={:?} | auth={:?}",
                agent.id,
                launch_display(&agent),
                agent.acp_command,
                agent.authentication
            );
        }
        if let Some(agent) = super::first_acp_available() {
            println!(
                "default ACP: {} => {}",
                agent.id,
                agent
                    .acp_command
                    .expect("selected agent offers ACP")
                    .join(" ")
            );
        } else {
            println!("default ACP: none");
        }
    }

    #[test]
    #[ignore = "measurement, not an assertion; run by hand with --ignored --nocapture"]
    fn reports_cli_launchability() {
        for agent in discover().agents {
            match super::probe_version(&agent) {
                Ok(output) => println!(
                    "{} => STARTED exit={:?} stdout={:?} stderr={:?}",
                    agent.id,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
                Err(error) => println!(
                    "{} => NOT STARTED path={} error={error}",
                    agent.id,
                    launch_display(&agent)
                ),
            }
        }
    }

    struct UnreadableDirectory {
        path: PathBuf,
        #[cfg(windows)]
        user: String,
        #[cfg(unix)]
        original_mode: std::fs::Permissions,
    }

    impl UnreadableDirectory {
        fn new(path: &Path) -> Self {
            #[cfg(windows)]
            {
                use std::ffi::OsStr;
                let user = String::from_utf8(
                    std::process::Command::new("whoami")
                        .output()
                        .expect("whoami")
                        .stdout,
                )
                .expect("whoami output")
                .trim()
                .to_string();
                let deny = format!("{user}:(OI)(CI)(RX)");
                let result = std::process::Command::new("icacls")
                    .args([path.as_os_str(), OsStr::new("/deny"), OsStr::new(&deny)])
                    .status()
                    .expect("icacls");
                assert!(result.success(), "icacls failed to deny directory access");
                Self {
                    path: path.to_path_buf(),
                    user,
                }
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let metadata = std::fs::metadata(path).expect("directory metadata");
                let original_mode = metadata.permissions();
                let mut denied = original_mode.clone();
                denied.set_mode(0);
                std::fs::set_permissions(path, denied).expect("remove directory permissions");
                Self {
                    path: path.to_path_buf(),
                    original_mode,
                }
            }
        }
    }

    impl Drop for UnreadableDirectory {
        fn drop(&mut self) {
            #[cfg(windows)]
            {
                use std::ffi::OsStr;
                let _ = std::process::Command::new("icacls")
                    .args([
                        self.path.as_os_str(),
                        OsStr::new("/remove:d"),
                        OsStr::new(&self.user),
                    ])
                    .status();
            }
            #[cfg(unix)]
            {
                let _ = std::fs::set_permissions(&self.path, self.original_mode.clone());
            }
        }
    }

    #[test]
    fn discover_counts_unreadable_path_directories() {
        let readable = temporary_directory("readable");
        std::fs::create_dir_all(&readable).expect("readable");
        let blocked = temporary_directory("blocked");
        std::fs::create_dir_all(&blocked).expect("blocked");
        let guard = UnreadableDirectory::new(&blocked);
        let discovery = super::discover_in_paths(&[readable.clone(), blocked.clone()]);
        let unreadable = discovery.unreadable_dirs;
        drop(guard);
        let _ = std::fs::remove_dir_all(&readable);
        let _ = std::fs::remove_dir_all(&blocked);
        assert_eq!(
            unreadable, 1,
            "an unreadable PATH directory must not be reported as missing"
        );
    }

    struct FixtureFetch;

    impl crate::registry::RegistryFetch for FixtureFetch {
        fn fetch_body(&self) -> Result<String, String> {
            Ok(crate::registry::TEST_REGISTRY_FIXTURE.to_string())
        }
    }

    struct CoverageFixtureFetch;

    impl crate::registry::RegistryFetch for CoverageFixtureFetch {
        fn fetch_body(&self) -> Result<String, String> {
            Ok(r#"{
  "agents": [
    {
      "id": "claude-acp",
      "distribution": {
        "npx": { "package": "claude-acp@1.0.0" }
      }
    },
    {
      "id": "codex-acp",
      "distribution": {
        "npx": {
          "package": "codex-acp@1.0.0",
          "args": ["--registry=https://evil"]
        }
      }
    },
    {
      "id": "pi-acp",
      "distribution": {
        "npx": { "package": "pi-acp@1.0.0" }
      }
    }
  ]
}"#
            .to_string())
        }
    }

    #[cfg(feature = "server")]
    #[test]
    fn registry_coverage_marks_only_claude_wrapper_unpickable_when_native_exists() {
        let dir = temporary_directory("registry-coverage-native");
        fs::create_dir_all(&dir).expect("temporary directory");
        fake_cli_path(&dir, "claude");
        fake_cli_path(&dir, "npx");
        let cache = temporary_directory("registry-coverage-native-cache");
        let catalog = super::discover_catalog_in_paths(
            &CoverageFixtureFetch,
            &cache,
            std::slice::from_ref(&dir),
        );

        let claude = catalog
            .agents
            .iter()
            .find(|agent| agent.id == "claude-acp")
            .expect("claude-acp from registry");
        assert_eq!(claude.pickable, Some(false));
        assert_eq!(claude.launch_args, Some(Vec::new()));
        assert_eq!(
            catalog
                .agents
                .iter()
                .find(|agent| agent.id == "codex-acp")
                .expect("codex-acp from registry")
                .launch_args,
            Some(vec!["--registry=https://evil".to_string()])
        );
        assert_eq!(
            catalog
                .agents
                .iter()
                .find(|agent| agent.id == "codex-acp")
                .expect("codex-acp from registry")
                .pickable,
            None
        );
        assert_eq!(
            catalog
                .agents
                .iter()
                .find(|agent| agent.id == "pi-acp")
                .expect("pi-acp from registry")
                .pickable,
            None
        );

        let no_native_dir = temporary_directory("registry-coverage-no-native");
        fs::create_dir_all(&no_native_dir).expect("temporary directory");
        fake_cli_path(&no_native_dir, "npx");
        let no_native_cache = temporary_directory("registry-coverage-no-native-cache");
        let no_native = super::discover_catalog_in_paths(
            &CoverageFixtureFetch,
            &no_native_cache,
            std::slice::from_ref(&no_native_dir),
        );
        assert_eq!(
            no_native
                .agents
                .iter()
                .find(|agent| agent.id == "claude-acp")
                .expect("claude-acp without native claude")
                .pickable,
            None
        );

        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(cache);
        let _ = fs::remove_dir_all(no_native_dir);
        let _ = fs::remove_dir_all(no_native_cache);
    }

    #[cfg(feature = "server")]
    #[test]
    fn native_installed_grok_drops_registry_grok_build() {
        let dir = temporary_directory("native-beats-registry");
        fs::create_dir_all(&dir).expect("temporary directory");
        fake_cli_path(&dir, "grok");
        let cache = temporary_directory("native-beats-cache");
        fs::create_dir_all(&cache).expect("cache");
        let catalog =
            super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
        let ids: Vec<&str> = catalog
            .agents
            .iter()
            .map(|agent| agent.id.as_str())
            .collect();
        assert!(ids.contains(&"grok"), "native grok must remain: {ids:?}");
        assert!(
            !ids.contains(&"grok-build"),
            "registry grok-build must not appear next to native grok: {ids:?}"
        );
        assert!(
            ids.contains(&"codex-acp"),
            "codex-acp has no native equivalent: {ids:?}"
        );
        let grok = catalog
            .agents
            .iter()
            .find(|agent| agent.id == "grok")
            .expect("grok");
        assert_eq!(grok.origin, super::ProviderOrigin::UserBinary);
        let codex = catalog
            .agents
            .iter()
            .find(|agent| agent.id == "codex-acp")
            .expect("codex-acp");
        assert_eq!(codex.origin, super::ProviderOrigin::NpxWrapper);
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(cache);
    }

    #[cfg(all(windows, feature = "server"))]
    #[test]
    fn npx_wrapper_argv_is_node_and_npx_cli_not_cmd() {
        let dir = temporary_directory("npx-unwrap");
        fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
        File::create(dir.join("node.exe")).expect("node");
        fs::write(
            dir.join("npx.cmd"),
            npm_cmd_shim_contents(r"node_modules\npm\bin\npx-cli.js"),
        )
        .expect("npx shim");
        fs::write(
            dir.join("node_modules")
                .join("npm")
                .join("bin")
                .join("npx-cli.js"),
            "/* npx */\n",
        )
        .expect("npx-cli.js");
        let cache = temporary_directory("npx-unwrap-cache");
        fs::create_dir_all(&cache).expect("cache");
        let catalog =
            super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
        let codex = catalog
            .agents
            .iter()
            .find(|agent| agent.id == "codex-acp")
            .expect("codex-acp from registry");
        let argv = codex
            .acp_command
            .as_ref()
            .expect("npx-wrapper must resolve an ACP command");
        assert!(
            !argv[0].to_ascii_lowercase().contains(".cmd"),
            "argv[0] must be node.exe, not a .cmd shim: {}",
            argv[0]
        );
        assert!(
            argv.iter()
                .any(|part| part.to_ascii_lowercase().ends_with("npx-cli.js")),
            "argv must include npx-cli.js: {argv:?}"
        );
        assert_eq!(argv[argv.len() - 2], "-y");
        assert_eq!(
            argv[argv.len() - 1],
            "@agentclientprotocol/codex-acp@1.10.0"
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(cache);
    }

    #[cfg(all(windows, feature = "server"))]
    #[test]
    fn npx_wrapper_real_launcher_shape_unwraps_to_node_and_npx_cli_js() {
        // Uses the real npm launcher shape (npx.cmd as shipped by npm 10.x),
        // NOT the per-package cmd-shim shape. The per-package shape test
        // above passed before the fix; this test with the REAL launcher
        // shape is the one that was red because the old code did not
        // recognize it.
        let dir = temporary_directory("npx-real-launcher");
        fs::create_dir_all(dir.join("node_modules").join("npm").join("bin")).expect("npm bin");
        File::create(dir.join("node.exe")).expect("node");
        fs::write(
            dir.join("npx.cmd"),
            "\
SET \"NODE_EXE=%~dp0\\node.exe\"\n\
IF NOT EXIST \"%NODE_EXE%\" ( SET \"NODE_EXE=node\" )\n\
SET \"NPM_PREFIX_JS=%~dp0\\node_modules\\npm\\bin\\npm-prefix.js\"\n\
SET \"NPX_CLI_JS=%~dp0\\node_modules\\npm\\bin\\npx-cli.js\"\n\
FOR /F \"delims=\" %%F IN ('CALL \"%NODE_EXE%\" \"%NPM_PREFIX_JS%\"') DO ( SET \"NPM_PREFIX_NPX_CLI_JS=%%F\\node_modules\\npm\\bin\\npx-cli.js\" )\n\
IF EXIST \"%NPM_PREFIX_NPX_CLI_JS%\" ( SET \"NPX_CLI_JS=%NPM_PREFIX_NPX_CLI_JS%\" )\n\
\"%NODE_EXE%\" \"%NPX_CLI_JS%\" %*\n",
        )
        .expect("npx.cmd");
        fs::write(
            dir.join("node_modules")
                .join("npm")
                .join("bin")
                .join("npx-cli.js"),
            "/* npx */\n",
        )
        .expect("npx-cli.js");
        let cache = temporary_directory("npx-real-launcher-cache");
        fs::create_dir_all(&cache).expect("cache");
        let catalog =
            super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
        let codex = catalog
            .agents
            .iter()
            .find(|agent| agent.id == "codex-acp")
            .expect("codex-acp from registry");
        let argv = codex
            .acp_command
            .as_ref()
            .expect("npx-wrapper with real launcher shape must resolve an ACP command");
        assert!(
            !argv[0].to_ascii_lowercase().contains(".cmd"),
            "argv[0] must be node.exe, not a .cmd shim: {}",
            argv[0]
        );
        assert!(
            argv.iter()
                .any(|part| part.to_ascii_lowercase().ends_with("npx-cli.js")),
            "argv must include npx-cli.js: {argv:?}"
        );
        assert_eq!(argv[argv.len() - 2], "-y");
        assert_eq!(
            argv[argv.len() - 1],
            "@agentclientprotocol/codex-acp@1.10.0"
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(cache);
    }

    #[cfg(all(windows, feature = "server"))]
    #[test]
    fn npx_wrapper_command_is_none_when_npx_cmd_is_not_a_shim() {
        let dir = temporary_directory("npx-nonsim");
        fs::create_dir_all(&dir).expect("temporary directory");
        File::create(dir.join("npx.cmd")).expect("non-shim npx.cmd");
        let cache = temporary_directory("npx-nonsim-cache");
        fs::create_dir_all(&cache).expect("cache");
        let catalog =
            super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
        let codex = catalog
            .agents
            .iter()
            .find(|agent| agent.id == "codex-acp")
            .expect("codex-acp from registry");
        assert_eq!(
            codex.acp_command, None,
            "a leftover .cmd/.bat argv[0] is not an ACP agent: {:?}",
            codex.acp_command
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(cache);
    }

    #[cfg(feature = "server")]
    #[test]
    fn npx_wrapper_command_is_none_when_npx_is_absent_from_path() {
        let dir = temporary_directory("npx-absent");
        fs::create_dir_all(&dir).expect("temporary directory");
        let cache = temporary_directory("npx-absent-cache");
        fs::create_dir_all(&cache).expect("cache");
        let catalog =
            super::discover_catalog_in_paths(&FixtureFetch, &cache, std::slice::from_ref(&dir));
        let codex = catalog
            .agents
            .iter()
            .find(|agent| agent.id == "codex-acp")
            .expect("codex-acp from registry");
        assert_eq!(
            codex.acp_command, None,
            "npx missing from PATH must not invent an ACP command: {:?}",
            codex.acp_command
        );
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(cache);
    }
}
