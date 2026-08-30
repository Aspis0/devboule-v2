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

use std::path::PathBuf;

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
/// Rejection is on the *raw* segments, before any filesystem call: a check that
/// canonicalizes first can be defeated by a symlink, and one that trusts the
/// operating system to normalise is trusting the wrong layer.
fn safe_relative_path(uri_path: &str) -> Option<String> {
    let trimmed = uri_path.trim_start_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let decoded = percent_decode(trimmed)?;
    let mut parts: Vec<&str> = Vec::new();
    for segment in decoded.split('/') {
        match segment {
            "" | "." => continue,
            ".." => return None,
            other if other.contains('\\') || other.contains(':') => return None,
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

fn plugins_root<R: tauri::Runtime>(context: &UriSchemeContext<'_, R>) -> Option<PathBuf> {
    context
        .app_handle()
        .path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("plugins"))
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
    let Some(root) = plugins_root(&context) else {
        respond(
            responder,
            StatusCode::INTERNAL_SERVER_ERROR,
            "text/plain",
            b"no plugin directory on this machine".to_vec(),
        );
        return;
    };

    let target = root.join(&relative);
    match std::fs::read(&target) {
        Ok(bytes) => respond(
            responder,
            StatusCode::OK,
            content_type_for(&relative),
            bytes,
        ),
        Err(_) => respond(
            responder,
            StatusCode::NOT_FOUND,
            "text/plain",
            b"no such plugin asset".to_vec(),
        ),
    }
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
    fn a_module_is_served_as_javascript_because_the_browser_checks() {
        assert_eq!(content_type_for("ui/index.js"), "text/javascript");
        assert_eq!(content_type_for("ui/index.mjs"), "text/javascript");
        assert_eq!(content_type_for("atlas/city.png"), "image/png");
        assert_eq!(content_type_for("no-extension"), "application/octet-stream");
    }
}
