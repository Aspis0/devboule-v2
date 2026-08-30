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
//! ## Nothing is served that discovery did not verify
//!
//! Every request other than the self test is checked against
//! [`super::PluginRegistry`]: the plugin named by the first path segment must
//! have passed verification, and the rest of the path must be a file its
//! manifest listed. Without that, refusing a plugin would only ever be advice —
//! the window could load it anyway by asking for the file directly, and the
//! content-policy entry that lets this origin execute scripts would be pointing
//! at bytes nothing vouched for.
//!
//! Refused and absent share a status on purpose. Telling them apart turns this
//! handler into a way to ask what exists on disk.

use std::path::Path;

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder};

/// The registered scheme. On Windows this becomes `http://plugin.localhost/`.
pub const PLUGIN_SCHEME: &str = "plugin";

/// Reserved path that reports the transport works, with no plugin installed.
const SELF_TEST_PATH: &str = "__selftest.js";

/// The module served for [`SELF_TEST_PATH`]. Deliberately trivial: it proves
/// the fetch, the MIME type, the CORS header and the CSP entry all line up,
/// and nothing else.
const SELF_TEST_MODULE: &str = "export const pluginTransportWorks = true;\n";

fn content_type_for(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        // A module served with the wrong type is refused by the browser before
        // it is ever parsed, so this is not cosmetic.
        Some("js") | Some("mjs") => "text/javascript",
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
fn percent_decode(value: &str) -> Option<String> {
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

fn respond(responder: UriSchemeResponder, status: StatusCode, kind: &str, body: Vec<u8>) {
    let response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, kind)
        // ES modules are fetched in CORS mode even from the app's own window,
        // so without this a plugin module fails to load rather than 404ing.
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, "*")
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .expect("plugin asset response is always well formed");
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
        );
        return;
    }

    let Some(relative) = safe_relative_path(&path) else {
        respond(
            responder,
            StatusCode::BAD_REQUEST,
            "text/plain",
            b"rejected plugin asset path".to_vec(),
        );
        return;
    };
    let Some(root) = super::plugins_root(context.app_handle()) else {
        respond(
            responder,
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain",
            b"no plugin directory on this machine".to_vec(),
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
        );
        return;
    }

    match read_contained(&root, &relative) {
        Some(bytes) => respond(
            responder,
            StatusCode::OK,
            content_type_for(&relative),
            bytes,
        ),
        // One status for "not there" and for "not allowed", deliberately:
        // telling them apart turns this handler into a filesystem probe.
        None => respond(
            responder,
            StatusCode::NOT_FOUND,
            "text/plain",
            b"no such plugin asset".to_vec(),
        ),
    }
}

/// A file is read only if it really lives under `root` once every link is
/// followed, and only if it is small enough to hold in memory.
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
    const MAX_ASSET_BYTES: u64 = 64 * 1024 * 1024;

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
}
