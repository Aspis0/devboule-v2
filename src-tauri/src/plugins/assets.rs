//! Serving a plugin's own files to the WebView.
//!
//! M5 makes Polis installable by the user rather than compiled in, which means
//! its JavaScript and its sprite atlases live in a directory Devboule did not
//! build. The WebView cannot load them from disk — `file://` is a different,
//! opaque origin — so the app registers a URI scheme and serves them itself.
//!
//! ## What the platform actually does, verified rather than assumed
//!
//! Tauri documents the origin of a registered scheme as platform-dependent:
//! `http://<scheme>.localhost/<path>` on Windows and Android,
//! `<scheme>://localhost/<path>` on macOS, iOS and Linux. The repository
//! already showed the Windows shape — `tauri.conf.json` lists
//! `http://ipc.localhost` for Tauri's own IPC scheme.
//!
//! Two consequences follow, and both were nearly missed:
//!
//! 1. **It is a different origin from the app.** `script-src 'self'` does not
//!    cover it, so the policy needs an explicit entry for this scheme. That is a
//!    real widening, and a much narrower one than `'unsafe-eval'`: no string
//!    becomes executable, and the bytes come from a directory we resolved.
//! 2. **ES modules are fetched in CORS mode**, always, unlike classic scripts.
//!    Without `Access-Control-Allow-Origin` on the response, a dynamic
//!    `import()` fails with a CORS error rather than a 404, which is a
//!    confusing way to learn this. The header is set below.
//!
//! ## The self test
//!
//! `__selftest.js` is answered from memory, not from disk. The question "can
//! this WebView load a plugin module at all" has to be answerable before any
//! plugin is installed — which is the state the app ships in — and a probe that
//! needs a file present would only ever report on the file.
//!
//! ## The workspace exception
//!
//! Every request other than the self test is checked against
//! [`super::PluginRegistry`]: the plugin named by the first path segment must
//! have passed verification, and the rest of the path must be a file its
//! manifest listed. Without that, refusing a plugin would only ever be advice —
//! the window could load it anyway by asking for the file directly, and the
//! content-policy entry that lets this origin execute scripts would be pointing
//! at bytes nothing vouched for.
//!
//! The `__workspace` branch is an explicit exception to that invariant: its
//! files are user data under the confined `workspace.root`, not manifest files
//! with install-time digests. It rechecks the verified manifest's
//! `workspace.root` request and the effective grant on every request, confines
//! the current root on every request, and serves only inert MIME types.
//!
//! This is a same-origin transport. Relative to this asset server, all
//! installed plugins are one trust domain; the plugin id in the URL is
//! self-declared. The capability guard stops a plugin asking for the workspace
//! on its own behalf, but it is best-effort, not a boundary: it cannot stop one
//! plugin asking under another plugin's id. The real per-plugin closure is a
//! future non-forgeable host token delivered only to that plugin's frame via
//! `postMessage`; that is recorded as debt for the surface-registration work.
//! The CORS guard therefore closes access from other origins — the app window
//! and remote content — not access between plugin ids on this shared origin.
//!
//! Refused and absent share a status on purpose. Telling them apart turns this
//! handler into a way to ask what exists on disk; the workspace branch adds
//! only the deliberate `413` for a file above its per-request memory cap.

use std::io::Read;
use std::path::{Path, PathBuf};

use devboule_plugin_rpc::{confine_project_path, granted_capabilities, workspace_root_for_grant};
use devboule_protocol::caps;
use tauri::http::{header, Request, Response, StatusCode};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, UriSchemeResponder};

/// The registered scheme. On Windows this becomes `http://plugin.localhost/`.
pub const PLUGIN_SCHEME: &str = "plugin";

/// Reserved path that reports the transport works, with no plugin installed.
const SELF_TEST_PATH: &str = "__selftest.js";

/// The module served for [`SELF_TEST_PATH`]. Deliberately trivial: it proves
/// the fetch, the MIME type, the CORS header and the CSP entry all line up,
/// and nothing else.
const SELF_TEST_MODULE: &str = "export const pluginTransportWorks = true;\n";

/// Content-Security-Policy sent on plugin HTML documents.
///
/// Not yet measured against a real PixiJS page. If a plugin frame fails to
/// render, suspect this first: it is the thing that changed between "the
/// bytes arrived" and "the page would not run".
const PLUGIN_DOCUMENT_CSP: &str = "\
default-src 'self'; \
script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; \
connect-src 'self'; \
worker-src 'self' blob:; \
frame-ancestors http://localhost:1420 http://tauri.localhost; \
base-uri 'none'; \
form-action 'none'; \
object-src 'none'";

fn content_security_policy_for(kind: &str) -> Option<&'static str> {
    (kind == "text/html").then_some(PLUGIN_DOCUMENT_CSP)
}

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        // A module served with the wrong type is refused by the browser before
        // it is ever parsed, so this is not cosmetic.
        Some("js") | Some("mjs") => "text/javascript",
        // The plugin frame loads entry.ui as a document. octet-stream would
        // render a blank frame, which is how this case was found: by reading,
        // not by a failing test.
        Some("html") | Some("htm") => "text/html",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("wasm") => "application/wasm",
        Some("png") => "image/png",
        Some("webp") => "image/webp",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
}

/// This is a per-request host-memory ceiling, not a statement about the
/// largest workspace file that may exist. Microscopy formats such as OME, LIF
/// and ND2 commonly exceed it; the cap is a per-request transport decision,
/// not a restriction on what the workspace may contain.
pub(super) const MAX_WORKSPACE_ASSET_BYTES: u64 = 8 * 1024 * 1024;

/// The workspace branch has no executable MIME type in its codomain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceContentType {
    Png,
    Jpeg,
    Webp,
    Gif,
    Bmp,
    Tiff,
    OctetStream,
}

impl WorkspaceContentType {
    #[cfg(test)]
    const ALL: [Self; 7] = [
        Self::Png,
        Self::Jpeg,
        Self::Webp,
        Self::Gif,
        Self::Bmp,
        Self::Tiff,
        Self::OctetStream,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Gif => "image/gif",
            Self::Bmp => "image/bmp",
            Self::Tiff => "image/tiff",
            Self::OctetStream => "application/octet-stream",
        }
    }
}

/// Workspace content types are a deliberately separate allowlist. Its return
/// type makes `text/html`, script types and SVG unrepresentable here for every
/// input; unknown and non-raster names fall through to `octet-stream`.
fn workspace_content_type_for(path: &str) -> WorkspaceContentType {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some(extension) if extension.eq_ignore_ascii_case("png") => WorkspaceContentType::Png,
        Some(extension)
            if extension.eq_ignore_ascii_case("jpg") || extension.eq_ignore_ascii_case("jpeg") =>
        {
            WorkspaceContentType::Jpeg
        }
        Some(extension) if extension.eq_ignore_ascii_case("webp") => WorkspaceContentType::Webp,
        Some(extension) if extension.eq_ignore_ascii_case("gif") => WorkspaceContentType::Gif,
        Some(extension) if extension.eq_ignore_ascii_case("bmp") => WorkspaceContentType::Bmp,
        Some(extension)
            if extension.eq_ignore_ascii_case("tif") || extension.eq_ignore_ascii_case("tiff") =>
        {
            WorkspaceContentType::Tiff
        }
        _ => WorkspaceContentType::OctetStream,
    }
}

/// Strip the request path out of the URI and reject anything that could climb
/// out of the plugin directory.
///
/// This is the FIRST of two checks and is not sufficient alone. It rejects the
/// *syntax* of an escape — `..`, backslashes, drive colons, control characters,
/// names Windows silently rewrites — before any filesystem call. It cannot see a
/// symlink, because a link is not in the syntax; that is what the containment
/// check in [`read_contained`] is for.
///
/// An earlier version of this comment claimed segment checking sufficed
/// *because* canonicalising can be defeated by a symlink. That was confused:
/// canonicalising is exactly how a symlink is caught, provided the result is
/// then tested for containment.
fn safe_relative_path(uri_path: &str) -> Option<String> {
    let trimmed = uri_path.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    safe_relative_segments(&percent_decode(trimmed)?)
}

/// The same check without the percent-decoding, for paths that never travelled
/// through a URL.
///
/// The manifest uses this one, and the split is not cosmetic: a manifest path is
/// written as-is, so decoding it would turn a file genuinely named `a%20b.js`
/// into `a b.js`. The verifier would hash one file and the server would serve
/// another, which is the sort of disagreement that only ever shows up on the one
/// plugin that has an odd file name.
pub(super) fn safe_relative_segments(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            // Backslash and colon are path syntax on Windows. A control
            // character or a NUL is either an encoder bug or an attempt to
            // truncate the name at a layer below this one.
            other
                if other.contains('\\')
                    || other.contains(':')
                    || other.chars().any(char::is_control) =>
            {
                return None
            }
            // Windows strips trailing dots and spaces, so `a.js.` names the
            // same file as `a.js` while comparing differently here.
            other if other.ends_with('.') || other.ends_with(' ') => return None,
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Minimal percent-decoding. Enough for the file names a plugin ships, and it
/// refuses malformed input instead of guessing at it.
pub(super) fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = value.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Split the reserved branch only after the common URL grammar has normalised
/// the request. A plugin file named `__workspace-other` is not this branch.
fn workspace_relative_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(super::WORKSPACE_ASSET_SEGMENT)?;
    if rest.is_empty() {
        Some("")
    } else {
        rest.strip_prefix('/')
    }
}

/// Resolve the same workspace grant that the backend spawn path resolves.
///
/// This is intentionally done for every HTTP request. It reads the verified
/// manifest from the plugin registry, obtains the current root from the
/// managed `OracleRuntime`, confines it with the shared RPC helper, and then
/// asks the shared capability grant function whether `workspace.root` would
/// actually be granted. Any missing state is absence, never a fallback root.
fn workspace_root_for_request<R: Runtime>(app: &AppHandle<R>, plugin_id: &str) -> Option<PathBuf> {
    let plugins_root = super::plugins_root(app)?;
    let registry = app.try_state::<super::PluginRegistry>()?;
    let manifest = registry.ready_manifest(&plugins_root, plugin_id)?;
    if !manifest
        .capabilities
        .iter()
        .any(|capability| capability == caps::WORKSPACE_ROOT)
    {
        return None;
    }

    let runtime = app.try_state::<crate::oracle::OracleRuntime>()?;
    let workspace = runtime.workspace().path?;
    let confined = confine_project_path(Path::new(&workspace)).ok()?;
    let grant_root = workspace_root_for_grant(&confined);
    let (_, grants) = granted_capabilities(&manifest.capabilities, Some(&grant_root));
    grants
        .contains_key(caps::WORKSPACE_ROOT)
        .then_some(confined)
}

fn response(
    status: StatusCode,
    kind: &str,
    body: Vec<u8>,
    allow_cross_origin: bool,
) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, kind)
        // Keep this invariant on every response, including errors and the
        // public bundle branch: the browser must not MIME-sniff asset bytes.
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CACHE_CONTROL, "no-store");
    if allow_cross_origin {
        // ES modules are fetched in CORS mode even from the app's own window,
        // so without this a bundle module fails to load rather than 404ing.
        // The workspace branch deliberately does not set ACAO.
        builder = builder.header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*");
    }
    if let Some(policy) = content_security_policy_for(kind) {
        builder = builder.header(header::CONTENT_SECURITY_POLICY, policy);
    }
    builder
        .body(body)
        .expect("plugin asset response is always well formed")
}

fn respond(
    responder: UriSchemeResponder,
    status: StatusCode,
    kind: &str,
    body: Vec<u8>,
    allow_cross_origin: bool,
) {
    let response = response(status, kind, body, allow_cross_origin);
    responder.respond(response);
}

/// Serve one request for a plugin file.
fn handle<R: tauri::Runtime>(
    context: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let path = request.uri().path().to_string();

    if path.trim_start_matches('/') == SELF_TEST_PATH {
        respond(
            responder,
            StatusCode::OK,
            "text/javascript",
            SELF_TEST_MODULE.as_bytes().to_vec(),
            true,
        );
        return;
    }

    let Some(relative) = safe_relative_path(&path) else {
        respond(
            responder,
            StatusCode::BAD_REQUEST,
            "text/plain",
            b"rejected plugin asset path".to_vec(),
            true,
        );
        return;
    };

    // Every asset belongs to a plugin, and the first segment names it. A file
    // sitting loose at the plugins root has no manifest vouching for it, so
    // there is nothing that could authorise serving it.
    let Some((plugin_id, inside)) = relative.split_once('/') else {
        respond(
            responder,
            StatusCode::NOT_FOUND,
            "text/plain",
            b"no such plugin asset".to_vec(),
            true,
        );
        return;
    };

    if let Some(workspace_relative) = workspace_relative_path(inside) {
        let Some(workspace_root) = workspace_root_for_request(context.app_handle(), plugin_id)
        else {
            // Missing state, no active workspace, a refused root, and a
            // plugin without an effective grant are all deliberately absent.
            respond(
                responder,
                StatusCode::NOT_FOUND,
                "text/plain",
                b"no such plugin asset".to_vec(),
                false,
            );
            return;
        };
        match read_workspace_asset(&workspace_root, workspace_relative) {
            Some(WorkspaceAsset::Bytes(bytes)) => respond(
                responder,
                StatusCode::OK,
                workspace_content_type_for(workspace_relative).as_str(),
                bytes,
                false,
            ),
            Some(WorkspaceAsset::TooLarge) => respond(
                responder,
                StatusCode::PAYLOAD_TOO_LARGE,
                "text/plain",
                // This 413 is not a new filesystem oracle: only a plugin with
                // an effective workspace.root grant reaches this branch, and
                // its backend already runs as the user and can read the tree.
                // Unlike the bundle branch, the status can therefore explain
                // the per-request memory cap without becoming a filesystem oracle.
                b"workspace asset exceeds the 8 MiB per-request cap".to_vec(),
                false,
            ),
            None => respond(
                responder,
                StatusCode::NOT_FOUND,
                "text/plain",
                b"no such plugin asset".to_vec(),
                false,
            ),
        }
        return;
    }

    let Some(root) = super::plugins_root(context.app_handle()) else {
        respond(
            responder,
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain",
            b"no plugin directory on this machine".to_vec(),
            true,
        );
        return;
    };
    // Fail closed on every branch: a registry that is not there yet, a plugin
    // that did not verify, a path its manifest never listed. Without this the
    // verification would be advice — the window could load a refused plugin
    // simply by asking for it.
    let verified = context
        .app_handle()
        .try_state::<super::PluginRegistry>()
        .is_some_and(|registry| registry.is_verified_asset(&root, plugin_id, inside));
    if !verified {
        respond(
            responder,
            StatusCode::NOT_FOUND,
            "text/plain",
            b"no such plugin asset".to_vec(),
            true,
        );
        return;
    }

    match read_plugin_asset(&root, plugin_id, inside) {
        Some(bytes) => respond(
            responder,
            StatusCode::OK,
            content_type_for(&relative),
            bytes,
            true,
        ),
        // One status for "not there" and for "not allowed", deliberately:
        // telling them apart turns this handler into a filesystem probe.
        None => respond(
            responder,
            StatusCode::NOT_FOUND,
            "text/plain",
            b"no such plugin asset".to_vec(),
            true,
        ),
    }
}

/// Serve one file of one plugin, following links only as far as that plugin's
/// own directory.
pub(super) fn read_plugin_asset(
    plugins_root: &Path,
    plugin_id: &str,
    inside: &str,
) -> Option<Vec<u8>> {
    read_contained(&plugins_root.join(plugin_id), inside)
}

enum WorkspaceAsset {
    Bytes(Vec<u8>),
    TooLarge,
}

/// Read a workspace file only after canonical containment, with a bounded read
/// as well as a metadata check so a file growing between those operations
/// cannot turn the host-memory cap into a race.
fn read_workspace_asset(root: &Path, relative: &str) -> Option<WorkspaceAsset> {
    if relative.is_empty() {
        return None;
    }
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let target = std::fs::canonicalize(canonical_root.join(relative)).ok()?;
    if !target.starts_with(&canonical_root) {
        return None;
    }
    let file = std::fs::File::open(&target).ok()?;
    // Check and use must operate on the same open handle, not on two
    // resolutions of the same path; reordering this for readability restores
    // the TOCTOU race this branch is meant to avoid.
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    if metadata.len() > MAX_WORKSPACE_ASSET_BYTES {
        return Some(WorkspaceAsset::TooLarge);
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut limited = file.take(MAX_WORKSPACE_ASSET_BYTES + 1);
    limited.read_to_end(&mut bytes).ok()?;
    if bytes.len() as u64 > MAX_WORKSPACE_ASSET_BYTES {
        Some(WorkspaceAsset::TooLarge)
    } else {
        Some(WorkspaceAsset::Bytes(bytes))
    }
}

/// A single asset larger than this is not held in memory to answer a request.
/// Discovery uses the same ceiling so a plugin that could never be served is
/// refused at verification, with a sentence, instead of 404ing at load time.
pub(super) const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

/// A file is read only if it really lives under `root` once every link is
/// followed, and only if it is small enough to hold in memory.
///
/// `root` is **this plugin's directory**, not the plugins root. A link from
/// plugin A to a file in plugin B would pass a containment check against the
/// shared root and be served in A's origin.
///
/// The syntactic check in [`safe_relative_path`] cannot see a symlink or an
/// NTFS junction — a link is not in the syntax. Someone able to write into the
/// plugin directory could otherwise drop a link to any file the app can read
/// and have it served back. Both sides are canonicalised so the comparison is
/// between real locations, and a file that escapes is treated exactly like a
/// file that is absent.
///
/// The ceiling is not incidental: the whole file is read into memory to answer
/// one request, so without it a large asset — hostile or merely careless — is
/// an out-of-memory in the app process.
fn read_contained(root: &Path, relative: &str) -> Option<Vec<u8>> {
    let canonical_root = std::fs::canonicalize(root).ok()?;
    let target = std::fs::canonicalize(canonical_root.join(relative)).ok()?;
    if !target.starts_with(&canonical_root) {
        return None;
    }
    let metadata = std::fs::metadata(&target).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_ASSET_BYTES {
        return None;
    }
    std::fs::read(&target).ok()
}

/// Register the scheme on the builder.
pub fn register<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
    builder.register_asynchronous_uri_scheme_protocol(PLUGIN_SCHEME, handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_that_climbs_out_is_refused() {
        for hostile in [
            "/../secrets.txt",
            "/a/../../b.js",
            "/..%2fescape.js",
            "/%2e%2e/escape.js",
            "/c:/windows/system32/x.js",
            "/a\\b.js",
            "/",
            "",
        ] {
            assert!(
                safe_relative_path(hostile).is_none(),
                "{hostile} should not resolve to a servable path"
            );
        }
    }

    #[test]
    fn an_ordinary_path_survives_intact() {
        assert_eq!(
            safe_relative_path("/polis/ui/index.js").as_deref(),
            Some("polis/ui/index.js")
        );
        assert_eq!(
            safe_relative_path("//polis///atlas/city.png").as_deref(),
            Some("polis/atlas/city.png")
        );
        assert_eq!(
            safe_relative_path("/polis/a%20b.js").as_deref(),
            Some("polis/a b.js")
        );
        // One decode: `%2520` is a file whose name contains `%20`, not a space.
        assert_eq!(
            safe_relative_path("/polis/a%2520b.js").as_deref(),
            Some("polis/a%20b.js")
        );
        // NUL after decoding is a control character, not a path.
        assert!(
            safe_relative_path("/polis/a%00b.js").is_none(),
            "a NUL in the path must not become a file name"
        );
    }

    #[test]
    fn malformed_escapes_are_refused_rather_than_guessed() {
        assert!(percent_decode("%").is_none());
        assert!(percent_decode("%zz").is_none());
        assert_eq!(percent_decode("plain").as_deref(), Some("plain"));
    }

    #[test]
    fn a_link_out_of_the_plugin_directory_is_not_served() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        std::fs::create_dir_all(&root).unwrap();
        let outside = temp.path().join("private.txt");
        std::fs::write(&outside, b"a secret the app can read").unwrap();
        std::fs::write(
            root.join("inside.js"),
            b"export const x = 1;
",
        )
        .unwrap();

        assert!(
            read_contained(&root, "inside.js").is_some(),
            "an ordinary file inside the directory must still be served"
        );
        // A traversal that survived the syntax check would land here.
        assert!(read_contained(&root, "../private.txt").is_none());

        // The link is the case the syntax check cannot see at all.
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_file(&outside, root.join("link.txt")).is_ok();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&outside, root.join("link.txt")).is_ok();
        if linked {
            assert!(
                read_contained(&root, "link.txt").is_none(),
                "a symlink pointing out of the plugin directory was served"
            );
        } else {
            // Creating a symlink on Windows needs privilege; skipping is
            // honest, silently passing would not be.
            eprintln!("skipped the symlink case: this machine would not create one");
        }
    }

    #[test]
    fn workspace_asset_reads_are_contained_and_bounded() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspace");
        std::fs::create_dir_all(root.join("images")).unwrap();
        std::fs::write(root.join("images/sample.tif"), b"pixels").unwrap();
        std::fs::create_dir(root.join("directory")).unwrap();
        let outside = temp.path().join("outside.bin");
        std::fs::write(&outside, b"outside").unwrap();

        assert!(matches!(
            read_workspace_asset(&root, "images/sample.tif"),
            Some(WorkspaceAsset::Bytes(bytes)) if bytes == b"pixels"
        ));
        assert!(read_workspace_asset(&root, "directory").is_none());
        assert!(read_workspace_asset(&root, "../outside.bin").is_none());

        let oversized = root.join("images/oversized.nd2");
        std::fs::File::create(&oversized)
            .unwrap()
            .set_len(MAX_WORKSPACE_ASSET_BYTES + 1)
            .unwrap();
        assert!(matches!(
            read_workspace_asset(&root, "images/oversized.nd2"),
            Some(WorkspaceAsset::TooLarge)
        ));
    }

    #[test]
    fn a_link_into_another_plugin_is_not_served() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        let alpha = root.join("alpha");
        let beta = root.join("beta");
        std::fs::create_dir_all(alpha.join("ui")).unwrap();
        std::fs::create_dir_all(&beta).unwrap();
        std::fs::write(alpha.join("ui/index.html"), b"<p>alpha</p>").unwrap();
        std::fs::write(beta.join("secret.js"), b"export const stolen = 1;\n").unwrap();

        #[cfg(windows)]
        let linked = std::process::Command::new("cmd")
            .arg("/c")
            .arg("mklink")
            .arg("/J")
            .arg(alpha.join("stolen"))
            .arg(&beta)
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&beta, alpha.join("stolen")).is_ok();
        if !linked {
            eprintln!("skipped the cross-plugin link case: this machine would not create one");
            return;
        }

        assert_eq!(
            read_plugin_asset(&root, "alpha", "ui/index.html").as_deref(),
            Some(&b"<p>alpha</p>"[..]),
            "an ordinary file of alpha must still be served"
        );
        assert!(
            read_plugin_asset(&root, "alpha", "stolen/secret.js").is_none(),
            "beta's bytes were served in alpha's origin"
        );
    }

    #[test]
    fn a_directory_is_not_an_asset() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("plugins");
        std::fs::create_dir_all(root.join("ui")).unwrap();
        assert!(read_contained(&root, "ui").is_none());
    }

    #[test]
    fn a_module_is_served_as_javascript_because_the_browser_checks() {
        assert_eq!(content_type_for("ui/index.js"), "text/javascript");
        assert_eq!(content_type_for("ui/index.mjs"), "text/javascript");
        assert_eq!(content_type_for("atlas/city.png"), "image/png");
        assert_eq!(content_type_for("no-extension"), "application/octet-stream");
    }

    #[test]
    fn workspace_content_type_codomain_contains_no_executable_type() {
        // The property is over the complete return type, not over a hand-picked
        // set of input names. An executable MIME string cannot be returned by
        // workspace_content_type_for because WorkspaceContentType has no such
        // variant.
        let executable = ["text/html", "text/javascript", "image/svg+xml"];
        for kind in WorkspaceContentType::ALL {
            assert!(
                !executable.contains(&kind.as_str()),
                "workspace MIME codomain unexpectedly contains {}",
                kind.as_str()
            );
        }
    }

    #[test]
    fn all_asset_responses_carry_nosniff_but_workspace_responses_do_not_cors() {
        let bundle = response(StatusCode::OK, "application/octet-stream", Vec::new(), true);
        let workspace = response(
            StatusCode::OK,
            "application/octet-stream",
            Vec::new(),
            false,
        );
        for asset_response in [&bundle, &workspace] {
            assert_eq!(
                asset_response
                    .headers()
                    .get(header::X_CONTENT_TYPE_OPTIONS)
                    .and_then(|value| value.to_str().ok()),
                Some("nosniff")
            );
        }
        assert_eq!(
            bundle
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );
        assert!(workspace
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none());
    }

    #[test]
    fn a_document_is_served_as_html_because_the_frame_renders_it() {
        assert_eq!(content_type_for("ui/index.html"), "text/html");
        assert_eq!(content_type_for("ui/index.htm"), "text/html");
    }

    #[test]
    fn a_plugin_document_carries_a_csp_that_closes_the_network() {
        let policy = content_security_policy_for("text/html")
            .expect("a plugin document must send a Content-Security-Policy");
        assert!(
            policy.contains("connect-src 'self'"),
            "connect-src 'self' is what stops a plugin phoning home: {policy}"
        );
        assert!(
            policy.contains("frame-ancestors http://localhost:1420 http://tauri.localhost"),
            "only the app may frame it: {policy}"
        );
        assert!(policy.contains("base-uri 'none'"), "{policy}");
        assert!(policy.contains("form-action 'none'"), "{policy}");
        assert!(policy.contains("object-src 'none'"), "{policy}");
        assert!(
            policy.contains("'wasm-unsafe-eval'"),
            "PixiJS/WebGL may compile wasm: {policy}"
        );
        assert!(
            policy.contains("'unsafe-inline'") && policy.contains("style-src"),
            "bundlers inject styles: {policy}"
        );
        assert!(
            policy.contains("data:") && policy.contains("blob:"),
            "textures arrive as data/blob URLs: {policy}"
        );
        let tokens: Vec<&str> = policy
            .split([' ', ';'])
            .filter(|token| !token.is_empty())
            .collect();
        assert!(
            !tokens.contains(&"'unsafe-eval'"),
            "plain unsafe-eval is not the wasm token: {policy}"
        );
    }

    #[test]
    fn assets_that_are_not_documents_do_not_carry_a_csp() {
        for kind in [
            "text/javascript",
            "text/css",
            "image/png",
            "application/json",
            "application/octet-stream",
        ] {
            assert_eq!(
                content_security_policy_for(kind),
                None,
                "{kind} is not a browsing context"
            );
        }
    }
}
