//! Images a provider produced: parsed from its frames and materialised into
//! the session's attachment store, so the transcript carries references the
//! same way prompt attachments do.
//!
//! Behaviour follows Paseo `codex-app-server-agent.ts` / `provider-image-output.ts`
//! (commit 4ed13fadb, Apache-2.0): `savedPath` wins over a `result`, a data
//! URL or bare base64 is bytes, any other `result` string is a URL, and a
//! failed generation is a tool row rather than an image.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use devboule_protocol::{AttachmentReference, PromptAttachment};
use serde_json::Value;

use crate::attachment_store::{extension_for, AttachmentStore};
use crate::raster_metadata::{sniff_raster_mime, RasterMime};

/// One image's ceiling, before decoding or reading. Well under the store's
/// 20 MiB per-owner budget, which the deposit then charges.
pub(crate) const MAX_AGENT_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// A URL fetch's own ceiling: no provider-named origin may hold a session's
/// ingest open indefinitely.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

/// One provider-named image before it is stored. `Base64` carries whatever
/// type the provider declared beside the bytes, if any.
#[derive(Debug)]
pub(crate) enum AgentImageSource {
    Path(PathBuf),
    Url(String),
    Base64 {
        mime_type: Option<String>,
        data: String,
    },
}

/// The store, the session, and the one root a provider-named path may live in.
pub(crate) struct AgentImageSink {
    store: AttachmentStore,
    session_id: String,
    workspace: PathBuf,
}

impl AgentImageSink {
    pub(crate) fn new(store: AttachmentStore, session_id: String, workspace: PathBuf) -> Self {
        Self {
            store,
            session_id,
            workspace,
        }
    }

    /// Store one source and answer its reference, or `None` when it is refused:
    /// an unknown container, a payload over the ceiling, a path outside the
    /// workspace and the temp dir, an unreadable file, or a fetch that failed.
    pub(crate) fn store(&self, source: &AgentImageSource) -> Option<AttachmentReference> {
        let bytes = match source {
            AgentImageSource::Path(path) => self.read_path(path)?,
            AgentImageSource::Url(url) => fetch_bounded(url)?,
            AgentImageSource::Base64 { data, .. } => decode_bounded(data)?,
        };
        let sniffed = sniff_raster_mime(&bytes)?;
        if let AgentImageSource::Base64 {
            mime_type: Some(declared),
            ..
        } = source
        {
            // The declared type is the sender's word; the bytes are the
            // evidence, and the store refuses a disagreement anyway.
            if RasterMime::from_mime_type(declared) != Some(sniffed) {
                return None;
            }
        }
        let mime_type = sniffed.as_mime_type();
        let extension = extension_for(mime_type)?;
        let attachment = PromptAttachment {
            name: format!("agent-image.{extension}"),
            mime_type: mime_type.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        };
        let deposited = self.store.deposit(&self.session_id, &attachment).ok()?;
        Some(AttachmentReference {
            session_id: self.session_id.clone(),
            digest: deposited.digest,
            stored_bytes: deposited.stored_bytes,
        })
    }

    /// A provider-named path is read only inside the session's workspace or
    /// the temp dir: the frame chooses neither, and an agent must not be able
    /// to name an arbitrary file of the machine into the transcript.
    fn read_path(&self, path: &Path) -> Option<Vec<u8>> {
        let resolved = path.canonicalize().ok()?;
        let allowed = [
            self.workspace.canonicalize().ok()?,
            std::env::temp_dir().canonicalize().ok()?,
        ];
        if !allowed.iter().any(|root| resolved.starts_with(root)) {
            return None;
        }
        read_bounded(&resolved)
    }
}

fn read_bounded(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_AGENT_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_AGENT_IMAGE_BYTES).then_some(bytes)
}

fn decode_bounded(data: &str) -> Option<Vec<u8>> {
    // Four base64 characters per three bytes, so an oversized payload is
    // refused before any of it is decoded.
    if data.len() > (MAX_AGENT_IMAGE_BYTES / 3 + 1) * 4 {
        return None;
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    (bytes.len() <= MAX_AGENT_IMAGE_BYTES).then_some(bytes)
}

fn fetch_bounded(url: &str) -> Option<Vec<u8>> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return None;
    }
    let client = reqwest::blocking::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .ok()?;
    let response = client.get(url).send().ok()?;
    if !response.status().is_success() {
        return None;
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_AGENT_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_AGENT_IMAGE_BYTES).then_some(bytes)
}

fn non_empty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// The `imageGeneration` / `imageView` source: `savedPath` (then `path`) wins
/// over a `result`, whose string form is a data URL, bare base64 or a URL and
/// whose object form carries `path`/`url`/`data` with an optional type.
pub(crate) fn codex_image_source(item: &Value) -> Option<AgentImageSource> {
    for key in ["savedPath", "saved_path", "path"] {
        if let Some(path) = non_empty(item, key) {
            return Some(AgentImageSource::Path(PathBuf::from(path)));
        }
    }
    let result = item.get("result")?;
    if let Some(text) = result.as_str() {
        return result_source(text);
    }
    for key in ["path", "savedPath", "saved_path"] {
        if let Some(path) = non_empty(result, key) {
            return Some(AgentImageSource::Path(PathBuf::from(path)));
        }
    }
    let declared = non_empty(result, "mimeType").or_else(|| non_empty(result, "mime_type"));
    if let Some(data) = non_empty(result, "data") {
        return Some(AgentImageSource::Base64 {
            mime_type: declared.map(str::to_string),
            data: data.to_string(),
        });
    }
    non_empty(result, "url").map(|url| AgentImageSource::Url(url.to_string()))
}

fn result_source(text: &str) -> Option<AgentImageSource> {
    if let Some(rest) = text.strip_prefix("data:") {
        let (meta, data) = rest.split_once(',')?;
        if data.is_empty() {
            return None;
        }
        let mime_type = meta.strip_suffix(";base64").filter(|mime| !mime.is_empty());
        return Some(AgentImageSource::Base64 {
            mime_type: mime_type.map(str::to_string),
            data: data.to_string(),
        });
    }
    let bare_base64 = text.len() > 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='));
    if bare_base64 {
        return Some(AgentImageSource::Base64 {
            mime_type: None,
            data: text.to_string(),
        });
    }
    Some(AgentImageSource::Url(text.to_string()))
}

/// One `{type:"image"}` content block: MCP's own `{data, mimeType}` or
/// Claude's `{source:{type:"base64", media_type, data}}`.
pub(crate) fn image_block_source(block: &Value) -> Option<AgentImageSource> {
    if block.get("type").and_then(Value::as_str) != Some("image") {
        return None;
    }
    if let Some(data) = non_empty(block, "data") {
        return Some(AgentImageSource::Base64 {
            mime_type: non_empty(block, "mimeType").map(str::to_string),
            data: data.to_string(),
        });
    }
    let source = block.get("source")?;
    if source.get("type").and_then(Value::as_str) == Some("base64") {
        let data = non_empty(source, "data")?;
        return Some(AgentImageSource::Base64 {
            mime_type: non_empty(source, "media_type").map(str::to_string),
            data: data.to_string(),
        });
    }
    non_empty(source, "url").map(|url| AgentImageSource::Url(url.to_string()))
}

#[cfg(test)]
#[path = "agent_image_tests.rs"]
mod tests;
