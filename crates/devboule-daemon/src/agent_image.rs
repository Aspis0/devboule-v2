//! Images a provider produced: the wire forms it names them in, the checks
//! that turn one into a stored reference, and the marker a prepared frame
//! carries instead of bytes, so the journal holds a reference once.
//!
//! Behaviour follows Paseo `codex-app-server-agent.ts` / `provider-image-output.ts`
//! (commit 4ed13fadb, Apache-2.0): `savedPath` wins over a `result`, a data
//! URL or bare base64 is bytes, any other `result` string is a URL, and a
//! failed generation is a tool row rather than an image.

use std::path::{Path, PathBuf};

use base64::Engine;
use devboule_protocol::{AttachmentReference, PromptAttachment};
use serde_json::{json, Value};

use crate::attachment_store::{extension_for, AttachmentStore};
use crate::checked_file::{open_checked, read_capped};
use crate::raster_metadata::{sniff_raster_mime, RasterMime};
use crate::session::MAX_ATTACHMENT_READ_BYTES;
use crate::workspace_git_support::{confined, walk, Walked};

/// The reason an image over the read cap is refused. The sink stores nothing
/// the reader cannot serve, so an image the row shows is an image the row can
/// fetch: both sides use [`MAX_ATTACHMENT_READ_BYTES`].
const TOO_LARGE: &str = "image too large";

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
/// the session's workspace, and the provider's own generated-image folder
/// when it has one.
pub(crate) struct AgentImageSink {
    store: AttachmentStore,
    session_id: String,
    workspace: PathBuf,
    images_dir: Option<PathBuf>,
}

impl AgentImageSink {
    /// The sink one provider session's frames are prepared through: the store
    /// under the session id the spawn puts in the child's environment, rooted
    /// at the folder it runs in. `images_dir` is the provider's own
    /// generated-image folder, and only Codex has one. A command with no
    /// session id keeps the sink out, and such a frame produces nothing, as
    /// it did before there was a store.
    pub(crate) fn for_spawn(
        store: AttachmentStore,
        env: &[(String, String)],
        workspace: PathBuf,
        images_dir: Option<PathBuf>,
    ) -> Option<Self> {
        let session_id = env
            .iter()
            .find(|(key, _)| key == crate::agent_env::SESSION_ID)?
            .1
            .clone();
        Some(Self::new(store, session_id, workspace, images_dir))
    }

    pub(crate) fn new(
        store: AttachmentStore,
        session_id: String,
        workspace: PathBuf,
        images_dir: Option<PathBuf>,
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
            name: attachment.name.clone(),
        })
    }

    /// A provider-named path is read only inside the session's workspace or
    /// the provider's own image folder. The path is walked under the root it
    /// sits in, so no component is a link, and the bytes come only from the
    /// handle that walk is bound to, read at most one byte past the cap.
    fn read_path(&self, path: &Path) -> Result<Vec<u8>, String> {
        let outside = || "the named file is outside the session's folders".to_string();
        let (root, relative) = self.root_of(path).ok_or_else(outside)?;
        let target = confined(&root, &relative).ok_or_else(outside)?;
        let walked = match walk(&root, &relative) {
            Walked::Inside(metadata) => metadata,
            Walked::Link(sentence) => return Err(sentence.to_string()),
            Walked::Missing => return Err("the named file could not be read".to_string()),
        };
        let (mut file, _) = open_checked(&root, &target, &walked)
            .map_err(|refusal| refusal.sentence().to_string())?;
        read_capped(&mut file, MAX_ATTACHMENT_READ_BYTES as u64)
            .map_err(|_| "the named file could not be read".to_string())?
            .ok_or_else(|| TOO_LARGE.to_string())
    }

    /// The root a provider-named path lies under and the path spelled from
    /// it. Both sides are canonical, so a spelling the OS resolves differently
    /// still lands in the root it names; the walk and the open then run on the
    /// root as configured.
    fn root_of(&self, path: &Path) -> Option<(PathBuf, String)> {
        let resolved = path.canonicalize().ok()?;
        let roots = std::iter::once(&self.workspace).chain(self.images_dir.as_ref());
        for root in roots {
            let Ok(canonical) = root.canonicalize() else {
                continue;
            };
            if let Ok(relative) = resolved.strip_prefix(&canonical) {
                return relative
                    .to_str()
                    .map(|relative| (root.clone(), relative.to_string()));
            }
        }
        None
    }
}

fn decode_bounded(data: &str) -> Result<Vec<u8>, String> {
    // Four base64 characters per three bytes, so an oversized payload is
    // refused before any of it is decoded.
    if data.len() > (MAX_ATTACHMENT_READ_BYTES / 3 + 1) * 4 {
        return Err(TOO_LARGE.to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| "the image's base64 could not be decoded".to_string())?;
    if bytes.len() > MAX_ATTACHMENT_READ_BYTES {
        return Err(TOO_LARGE.to_string());
    }
    Ok(bytes)
}

fn non_empty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// The key a prepared frame carries an image outcome in. A provider must not
/// be able to write one itself, so every incoming occurrence is dropped
/// before this daemon writes its own.
pub(crate) const MARKER: &str = "devboule_image";

/// The line a refused image leaves behind: the row says it had a picture
/// and why it has none, rather than showing one it does not carry.
fn not_stored(reason: &str) -> Value {
    json!({"type": "text", "text": format!("[image not stored: {reason}]")})
}

/// One image marker: the stored reference, the reason it was refused, or
/// that the item is still running.
pub(crate) fn marker(stored: StoredImage) -> Value {
    json!({ MARKER: stored.to_value() })
}

/// Store every image of one tool result and replace it with a marker, in
/// place: the frame the journal keeps holds each reference once instead of
/// the base64 again, and a replay shows the same pictures from that alone.
///
/// Both shapes a provider names an image in are read: a block that is the
/// image itself (MCP's `{data, mimeType}`, Claude's `{source: {…}}`) and an
/// ACP `content` element wrapping one. At most [`MAX_IMAGES_PER_FRAME`]
/// image blocks are stored; the rest become the not-stored line.
pub(crate) fn prepare_result_content(content: &mut Value, sink: &AgentImageSink) {
    match content.as_array_mut() {
        Some(blocks) => {
            let mut taken = 0;
            for block in blocks.iter_mut() {
                taken += prepare_block(block, taken, sink);
            }
        }
        None => {
            prepare_block(content, 0, sink);
        }
    }
}

/// How many of the frame's image budget this block took: one when it was an
/// image the store accepted, zero otherwise.
fn prepare_block(block: &mut Value, taken: usize, sink: &AgentImageSink) -> usize {
    forget_incoming_marker(block);
    let nested = block.get("type").and_then(Value::as_str) == Some("content");
    let target: &mut Value = if nested {
        let Some(content) = block.get_mut("content") else {
            return 0;
        };
        content
    } else {
        block
    };
    forget_incoming_marker(target);
    if target.get("type").and_then(Value::as_str) != Some("image") {
        return 0;
    }
    let Some(source) = image_block_source(target) else {
        return 0;
    };
    if taken >= MAX_IMAGES_PER_FRAME {
        *target = not_stored("more images than one frame stores");
        return 0;
    }
    *target = match sink.store(&source) {
        Ok(reference) => marker(StoredImage::Reference(reference)),
        Err(reason) => not_stored(&reason),
    };
    1
}

fn forget_incoming_marker(block: &mut Value) {
    if let Some(object) = block.as_object_mut() {
        object.remove(MARKER);
    }
}

/// The stored references a prepared result content carries, in order. A
/// block the store refused is a plain text line by then and is skipped, as
/// are the blocks that never named an image.
pub(crate) fn marked_result_images(content: &Value) -> Vec<AttachmentReference> {
    let blocks: &[Value] = match content.as_array() {
        Some(blocks) => blocks,
        None => std::slice::from_ref(content),
    };
    let mut references = Vec::new();
    for block in blocks {
        let target = match block.get("type").and_then(Value::as_str) {
            Some("content") => block.get("content").unwrap_or(&Value::Null),
            _ => block,
        };
        if let Some(StoredImage::Reference(reference)) =
            target.get(MARKER).and_then(StoredImage::from_value)
        {
            references.push(reference);
        }
    }
    references
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
