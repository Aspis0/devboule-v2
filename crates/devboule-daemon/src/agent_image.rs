//! Images a provider produced: the wire forms it names them in, the checks
//! that turn one into a stored reference, and the marker a prepared frame
//! carries instead of bytes, so the journal holds a reference once.
//!
//! Behaviour follows Paseo `codex-app-server-agent.ts` / `provider-image-output.ts`
//! (commit 4ed13fadb, Apache-2.0): `savedPath` wins over a `result`, a data
//! URL or bare base64 is bytes, any other `result` string is a URL, and a
//! failed generation is a tool row rather than an image.

use std::io::Read;
use std::path::{Path, PathBuf};

use base64::Engine;
use devboule_protocol::{AttachmentReference, PromptAttachment};
use serde_json::{json, Value};

use crate::attachment_store::{extension_for, AttachmentStore};
use crate::raster_metadata::{sniff_raster_mime, RasterMime};

/// One image's ceiling. Chosen against both budgets it sits under: four of
/// them fit the store's 20 MiB per-owner budget, and the base64 a frame would
/// carry stays under the reader's 10 MiB line cap.
pub(crate) const MAX_AGENT_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Images taken out of one frame. The rest of the blocks are left alone and
/// counted here so a hostile result cannot turn one frame into unbounded work.
pub(crate) const MAX_IMAGES_PER_FRAME: usize = 8;

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

/// What a prepared frame carries in place of an image source: the stored
/// reference, the reason it was refused, or that the item is still running.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StoredImage {
    Reference(AttachmentReference),
    Refused(String),
    Pending,
}

impl StoredImage {
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Self::Reference(reference) => json!({"reference": reference}),
            Self::Refused(reason) => json!({"refused": reason}),
            Self::Pending => json!({"pending": true}),
        }
    }

    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        if value.get("pending").and_then(Value::as_bool) == Some(true) {
            return Some(Self::Pending);
        }
        if let Some(reference) = value.get("reference") {
            return serde_json::from_value(reference.clone())
                .ok()
                .map(Self::Reference);
        }
        value
            .get("refused")
            .and_then(Value::as_str)
            .map(|reason| Self::Refused(reason.to_string()))
    }
}

/// The store, the session, and the roots a provider-named path may live in:
/// the session's workspace, and the provider's own generated-image folder.
pub(crate) struct AgentImageSink {
    store: AttachmentStore,
    session_id: String,
    workspace: PathBuf,
    images_dir: PathBuf,
}

impl AgentImageSink {
    pub(crate) fn new(
        store: AttachmentStore,
        session_id: String,
        workspace: PathBuf,
        images_dir: PathBuf,
    ) -> Self {
        Self {
            store,
            session_id,
            workspace,
            images_dir,
        }
    }

    /// Store one source, or say in a short sentence why it cannot be shown.
    /// This never touches the network: a URL is refused, not fetched.
    pub(crate) fn store(&self, source: &AgentImageSource) -> Result<AttachmentReference, String> {
        let bytes = match source {
            AgentImageSource::Path(path) => self.read_path(path)?,
            AgentImageSource::Url(url) => {
                return Err(format!("remote image not fetched ({url})"));
            }
            AgentImageSource::Base64 { data, .. } => decode_bounded(data)?,
        };
        let sniffed = sniff_raster_mime(&bytes)
            .ok_or_else(|| "the bytes are not an image this daemon stores".to_string())?;
        if let AgentImageSource::Base64 {
            mime_type: Some(declared),
            ..
        } = source
        {
            // The declared type is the sender's word; the bytes are the
            // evidence, and the store refuses a disagreement anyway.
            if RasterMime::from_mime_type(declared) != Some(sniffed) {
                return Err("the declared type does not match the bytes".to_string());
            }
        }
        let mime_type = sniffed.as_mime_type();
        let extension = extension_for(mime_type)
            .ok_or_else(|| "the image's container is not stored".to_string())?;
        let attachment = PromptAttachment {
            name: format!("agent-image.{extension}"),
            mime_type: mime_type.to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(&bytes),
        };
        let deposited = self
            .store
            .deposit(&self.session_id, &attachment)
            .map_err(|error| format!("the attachment store refused it ({})", error.message))?;
        Ok(AttachmentReference {
            session_id: self.session_id.clone(),
            digest: deposited.digest,
            stored_bytes: deposited.stored_bytes,
        })
    }

    /// A provider-named path is read only inside the session's workspace or
    /// the provider's own image folder, and only when it is a regular file:
    /// the frame chooses neither, and a FIFO or device would block the reader.
    fn read_path(&self, path: &Path) -> Result<Vec<u8>, String> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| "the named file could not be read".to_string())?;
        if !metadata.file_type().is_file() {
            return Err("the named path is not a regular file".to_string());
        }
        let resolved = path
            .canonicalize()
            .map_err(|_| "the named file could not be read".to_string())?;
        let allowed = [
            self.workspace.canonicalize().ok(),
            self.images_dir.canonicalize().ok(),
        ];
        if !allowed
            .iter()
            .flatten()
            .any(|root| resolved.starts_with(root))
        {
            return Err("the named file is outside the session's folders".to_string());
        }
        let file = std::fs::File::open(&resolved)
            .map_err(|_| "the named file could not be read".to_string())?;
        // The open is the check's subject as well: a file swapped between the
        // path check and here is still refused unless it is a regular file of
        // a readable size.
        let opened = file
            .metadata()
            .map_err(|_| "the named file could not be read".to_string())?;
        if !opened.is_file() {
            return Err("the named path is not a regular file".to_string());
        }
        if opened.len() > MAX_AGENT_IMAGE_BYTES as u64 {
            return Err("the image is over the 5 MiB limit".to_string());
        }
        let mut bytes = Vec::new();
        file.take(MAX_AGENT_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "the named file could not be read".to_string())?;
        if bytes.len() > MAX_AGENT_IMAGE_BYTES {
            return Err("the image is over the 5 MiB limit".to_string());
        }
        Ok(bytes)
    }
}

fn decode_bounded(data: &str) -> Result<Vec<u8>, String> {
    // Four base64 characters per three bytes, so an oversized payload is
    // refused before any of it is decoded.
    if data.len() > (MAX_AGENT_IMAGE_BYTES / 3 + 1) * 4 {
        return Err("the image is over the 5 MiB limit".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "the image's base64 could not be decoded".to_string())?;
    if bytes.len() > MAX_AGENT_IMAGE_BYTES {
        return Err("the image is over the 5 MiB limit".to_string());
    }
    Ok(bytes)
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
