//! Attachment and prompt planning: the `[Image available at: <path>]` lines a
//! prompt carries, the stored-reference resolution that appends to them, and
//! the ACP prompt plan - the image-delivery decision, the structured text
//! block, and the sinks a provider hands its client.
//!
//! Split out of `session_items.rs` without a rewrite: every line below this
//! header is byte-identical to its text there (lines 792-1315 of `7235ff8`,
//! where `session_items.rs` was 2268 lines), and not one visibility marker
//! changed. The four names the remainder still reads from here (`ImageDelivery`,
//! `AcpPromptSink`, `StaticImageSink`, `PlannedStaticPrompt`) are `pub(crate)`,
//! and the parent re-exports them, which is what leaves the `PtySession` and
//! `SpawnedSession` fields in `session_items.rs` naming them unqualified.

use super::*;

/// The prompt the writer receives: the user's text, a blank line, then one line
/// per attachment naming the absolute path its bytes were written to.
///
/// The line is Paseo's shape (`[Image available at: <path>]`), and every
/// attachment gets one — including an SVG, which no provider accepts as an
/// inline image block, so a path on disk is its only route to the agent both
/// now and after the per-provider blocks land. Nothing else about the prompt
/// changes, which is what lets the four provider writers stay untouched.
///
/// Every file is written before any of the text is built: a request that fails
/// on its third attachment leaves nothing to clean up out of the prompt that a
/// half-built string would otherwise have implied.
pub(super) fn with_attachment_paths(
    store: &AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[PromptAttachment],
) -> Result<String, WireError> {
    if attachments.is_empty() {
        return Ok(text.to_string());
    }
    let session = store
        .session(session_id)
        .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest, "Invalid session id."))?;
    let mut paths = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        paths.push(session.materialize(attachment)?);
    }
    let mut prompt = String::from(text);
    prompt.push_str("\n\n");
    push_path_lines(&mut prompt, &paths);
    Ok(prompt)
}

/// One `[Image available at: <path>]` line per path, separated by newlines.
///
/// The one place that line is written, so the inline attachments and the
/// resolved references cannot come out as two shapes: `with_attachment_paths`,
/// `prompt_text_with_fallback_paths` and `push_reference_path_lines` all write
/// their block through it, and each opens the block with its own separator.
fn push_path_lines(prompt: &mut String, paths: &[PathBuf]) {
    for (index, path) in paths.iter().enumerate() {
        if index > 0 {
            prompt.push('\n');
        }
        prompt.push_str("[Image available at: ");
        prompt.push_str(&path.to_string_lossy());
        prompt.push(']');
    }
}

/// Appends one path line per resolved reference to `prompt`, after whatever
/// path lines it already carries.
///
/// The references come last and in the order the client listed them: every
/// caller appends this after the inline attachments' own lines, which are the
/// paths this request's own bytes were written to. The block opens the way the
/// inline one does (`\n\n`), so a prompt that carries both reads as the inline
/// attachments first and the stored ones after them. An empty slice appends
/// nothing, and a prompt with no references is byte for byte what it was
/// before this existed.
///
/// # Why a reference is never an inline image block
///
/// Not an oversight, and not a missing case in the image-block routes: a
/// reference is a line here even on a provider that negotiated `image`
/// support. The whole reason a reference exists is that its bytes must not
/// travel in the frame — a deck is forty pages, and the frame is what the
/// deposit was made to keep them out of. Resolving one back into an image
/// block at the send would undo the deposit, spend the frame cap the deposit
/// saved, and hand the provider the same bytes by a longer road.
///
/// The path is the store's own absolute one. A reference carries a digest and
/// a size and nothing else, so there is no client-supplied name here to quote
/// and nothing untrusted to bound.
pub(super) fn push_reference_path_lines(prompt: &mut String, reference_paths: &[PathBuf]) {
    if reference_paths.is_empty() {
        return;
    }
    prompt.push_str("\n\n");
    push_path_lines(prompt, reference_paths);
}

/// The paths of the stored attachments this request names, or the first reason
/// one of them cannot be sent.
///
/// The order is the point, and it is the order `deposit` keeps. The wire's own
/// rule for references runs before any of this
/// (`validate_attachment_references`, called on the send path before this
/// function), so a reference naming another session, a digest that is not a
/// digest, and a list past the count or the total budget are refused without a
/// lookup. Then, per reference, the store resolves the digest to a path inside
/// this session's folder — its own read side, which refuses a session folder
/// that is a link and a digest with no file behind it.
///
/// The size is compared, and a disagreement is a refusal rather than a
/// warning. `resolve` answers with the size the file *has*; the reference's
/// `stored_bytes` is advisory by the wire's own documentation and is never the
/// number the daemon trusts. A request that names a size the file does not
/// have is naming something it did not deposit, and handing a provider a path
/// to a file whose identity is in question is the substitution this whole path
/// exists to prevent.
///
/// Every reference is resolved before the caller builds a line of the prompt,
/// for the reason [`with_attachment_paths`] gives about the inline ones: a
/// request that fails on its third item must leave nothing half-built.
pub(super) fn resolve_attachment_references(
    store: &AttachmentStore,
    session_id: &str,
    references: &[AttachmentReference],
) -> Result<Vec<PathBuf>, WireError> {
    let mut paths = Vec::with_capacity(references.len());
    for reference in references {
        // No extension hint: a reference carries a session, a digest and a
        // size, and no MIME type, so this caller knows nothing that would name
        // the file. The store's listing answers instead.
        let (path, stored_bytes) = store.resolve(session_id, &reference.digest, None)?;
        if stored_bytes != reference.stored_bytes {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                stored_size_mismatch_message(reference, stored_bytes),
            ));
        }
        paths.push(path);
    }
    Ok(paths)
}

/// The refusal for a reference whose `stored_bytes` is not the size the file
/// on disk has.
///
/// The digest is echoed and nothing else is: by the time this runs the digest
/// is 64 lowercase hex characters — `validate_attachment_references` refused
/// every other spelling before the store was asked, and `resolve` refuses a
/// non-digest again without echoing it — so there is no unbounded string here
/// to bound. Both numbers travel because together they are the whole
/// disagreement: which file, and how far the request's claim is from it.
fn stored_size_mismatch_message(reference: &AttachmentReference, stored_bytes: u64) -> String {
    format!(
        "The stored attachment '{}' is {stored_bytes} bytes; the request named {}.",
        reference.digest, reference.stored_bytes
    )
}

/// What the ACP handshake negotiated about sending images to the agent.
///
/// Two different kinds of fact: `NegotiatedImageBlock` came from this
/// session's `initialize` reply (`agentCapabilities.promptCapabilities.image`,
/// read by [`crate::acp_view::prompt_capabilities_from_initialize`]), while
/// `StaticImageBlock` is what this daemon knows about a provider whose
/// handshake says nothing — a fact about the protocol, not a fact the peer
/// agreed to. They are stored in one enum because the send path asks one
/// question (`may I send bytes?`), and that question must be answered the
/// same way whatever the source: only `Supported` is yes. `Absent` (the agent
/// said nothing) and `Unsupported` (the agent explicitly refused) are both
/// no, because Unknown must never silently mean yes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ImageDelivery {
    /// No image-capable route is available: fall back to the path line.
    #[default]
    PathLine,
    /// The ACP handshake negotiated `promptCapabilities.image == true` for
    /// this session. The only yes.
    NegotiatedImageBlock,
    /// A provider whose protocol carries images but whose handshake says
    /// nothing the daemon reads: Claude, Codex and Pi each answer with it, and
    /// their plans read it through this variant rather than against a literal.
    StaticImageBlock,
}

impl ImageDelivery {
    /// True only for a negotiated `Supported`. `Absent` and `Unsupported`
    /// both fall back to the path line: silence is not consent, and a
    /// refusal is not consent either.
    fn allows_image_block(state: crate::acp_view::PromptCapabilityState) -> bool {
        matches!(state, crate::acp_view::PromptCapabilityState::Supported)
    }

    /// Maps one handshake verdict to the delivery it authorises.
    pub(crate) fn from_negotiated(state: crate::acp_view::PromptCapabilityState) -> Self {
        if Self::allows_image_block(state) {
            Self::NegotiatedImageBlock
        } else {
            Self::PathLine
        }
    }
}

/// One image block for a `session/prompt` content array.
///
/// The bytes are the STRIPPED bytes — read back from the file
/// [`crate::attachment_store::SessionAttachments::materialize`] wrote, never
/// the base64 that arrived on the wire — so nothing leaving the house carries
/// identity metadata. The mime type is the stored attachment's declared type,
/// which `materialize` already checked against the sniffed container (a file
/// whose bytes and label disagree is refused, never stored).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AcpImageBlock {
    pub mime_type: String,
    pub data_base64: String,
}

/// Hand-written, and it must stay hand-written: `data_base64` holds a whole
/// image. One rendered PDF page is ~128 KiB of base64 and a deck is forty of
/// them, so a derived `Debug` would let any `{:?}` — a failing `assert_eq!`,
/// a log line, an error path, a future panic — spill the user's picture into
/// somewhere it was never meant to go. What a person debugging needs is the
/// type and the size; the bytes have never once been the answer.
impl std::fmt::Debug for AcpImageBlock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AcpImageBlock")
            .field("mime_type", &self.mime_type)
            .field("data_base64_len", &self.data_base64.len())
            .finish()
    }
}

impl AcpImageBlock {
    /// Reads the stripped file back and encodes it for the wire. Reading the
    /// file — rather than keeping a parallel copy of the pre-strip bytes — is
    /// what guarantees the block carries what is on disk.
    pub(super) fn from_stored_file(
        path: &std::path::Path,
        mime_type: &str,
    ) -> Result<Self, std::io::Error> {
        use base64::Engine;
        let bytes = std::fs::read(path)?;
        Ok(Self {
            mime_type: mime_type.to_string(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        })
    }

    pub(super) fn to_content_block(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "image",
            "mimeType": self.mime_type,
            "data": self.data_base64,
        })
    }
}

/// A structured sender for ACP prompts.
///
/// Two halves, one decision. The DECISION — which attachments become image
/// blocks, what text the journal records — is [`plan_structured_prompt`]: a
/// pure function of the request's `(text, attachments)`, tested directly
/// below without spawning a child. The DELIVERY — handing that plan to the
/// child — is [`AcpPromptSink::send_structured_prompt`], which needs the
/// live transport.
///
/// The plan carries both halves the send path needs: `fallback_text` (the
/// user's text plus the path lines for the non-raster attachments — today:
/// SVG, which no provider accepts inline) is the text block AND the exact
/// string the journal records, so the transcript can never carry image
/// base64; `images` are the blocks that travel. One constructor builds both,
/// so the journaled string and the sent text block cannot drift apart.
pub(crate) struct StructuredPromptPlan {
    /// The text block: the user's text plus one path line per non-raster
    /// attachment. Also the exact string the journal records.
    pub fallback_text: String,
    /// One block per raster attachment, in attachment order.
    pub images: Vec<AcpImageBlock>,
}

impl StructuredPromptPlan {
    /// The full `prompt` array the child receives: the text block, then one
    /// image block per raster attachment. The journal records
    /// `fallback_text` — element zero of this array — never the blocks.
    /// `pub(crate)` for the acp_client wire-shape test, which pins the
    /// exact JSON the read side already expects.
    pub(crate) fn content_blocks(&self) -> Vec<serde_json::Value> {
        let mut prompt = vec![serde_json::json!({ "type": "text", "text": self.fallback_text })];
        prompt.extend(self.images.iter().map(AcpImageBlock::to_content_block));
        prompt
    }
}

// HARD-WRITTEN ON PURPOSE — DO NOT REPLACE WITH `#[derive(Debug)]`.
//
// This struct carries the base64 of every attached image (one rendered PDF
// page is ~128 KiB, a deck is forty of them) and the user's own prompt text.
// A derived `Debug` would leave both exactly one `{:?}` away from a log line,
// a journal row, an assertion message or a future panic hook, and the standing
// rule from the earlier frame audit is that frame contents stay out of `Debug`
// output. This impl prints what a human debugging a prompt needs and nothing
// more: one `(mime type, base64 length in bytes)` pair per image block, plus
// the length of the text block. Never the base64, never the text.
//
// Each pair is a `&str` label and a `usize`, so this impl cannot copy base64
// into its output even by accident — the leak is excluded by the types, not by
// remembering. Note that `AcpImageBlock` above still derives `Debug`; do not
// route this impl (or anything else that renders a block) through it.
impl std::fmt::Debug for StructuredPromptPlan {
    /// The block as `(mime type, base64 byte length)`: the two facts a human
    /// needs to size a prompt up, and nothing that carries image bytes.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let images: Vec<(&str, usize)> = self
            .images
            .iter()
            .map(|block| (block.mime_type.as_str(), block.data_base64.len()))
            .collect();
        formatter
            .debug_struct("StructuredPromptPlan")
            .field("images", &images)
            .field("fallback_text_bytes", &self.fallback_text.len())
            .finish()
    }
}

/// Decides the structured prompt for one request: materializes every
/// attachment (exactly the call `with_attachment_paths` makes — a request
/// that fails on its third attachment leaves nothing half-built), turns each
/// raster (`image/png`, `image/jpeg`) into an image block carrying the
/// STRIPPED bytes read back from disk, and keeps every other attachment
/// (today: `image/svg+xml`) as a path line in the text. A prompt can
/// therefore carry both blocks and path lines at once. Returns `None` when
/// there is nothing to send inline (no attachments, or an SVG-only prompt),
/// in which case the caller takes the legacy path-line write.
pub(super) fn plan_structured_prompt(
    store: &AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[PromptAttachment],
) -> Result<Option<StructuredPromptPlan>, WireError> {
    if attachments.is_empty() {
        return Ok(None);
    }
    let session = store
        .session(session_id)
        .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest, "Invalid session id."))?;
    let mut images = Vec::new();
    let mut fallback_paths = Vec::new();
    for attachment in attachments {
        let path = session.materialize(attachment)?;
        if crate::raster_metadata::RasterMime::from_mime_type(&attachment.mime_type).is_some() {
            images.push(
                AcpImageBlock::from_stored_file(&path, &attachment.mime_type).map_err(|error| {
                    WireError::new(
                        ErrorCode::Io,
                        format!("Could not read a stored attachment: {error}"),
                    )
                })?,
            );
        } else {
            fallback_paths.push(path);
        }
    }
    if images.is_empty() {
        // SVG-only (or otherwise non-raster) on a capable session: nothing
        // would travel inline, so stay on the legacy write rather than
        // materializing twice — the fallback below re-materializes from the
        // content-addressed store, which is a wasted decode and strip, not a
        // double write, but there is no reason to pay it.
        return Ok(None);
    }
    Ok(Some(StructuredPromptPlan {
        fallback_text: prompt_text_with_fallback_paths(text, &fallback_paths),
        images,
    }))
}

/// The DELIVERY half: the text plus any image blocks land
/// as one `session/prompt` `prompt` array, instead of as a text blob with
/// path lines appended. It holds an `Arc` to the session's transport — the
/// same transport the plain-text [`Write`] half writes through — so both
/// halves share one session id, one request-id sequence, one pending table,
/// and the live negotiated capability slot.
///
/// The sibling is `Some` only for an ACP session; it is `None` for the other
/// three providers and for every terminal session. Whether it is *used* is a
/// second, per-prompt decision read from the negotiated capability (see
/// [`ImageDelivery`]): a present sibling with an Absent/Unsupported verdict
/// still falls back to the path line. The plain `Write` trait on `writer` is
/// untouched — text-only writes keep flowing through exactly the path they
/// use today.
///
/// There is deliberately no test seam here. One was written — a recording
/// double behind the delivery call — for a journal test on the structured
/// route that was never finished, and it sat unreachable: a seam shaped for
/// an imagined test, which is the shape least likely to fit the test someone
/// eventually writes. What the decision produces is pinned instead by
/// [`plan_structured_prompt`], which is pure and runs before anything is
/// sent, so the tests assert the exact value production would deliver. When
/// the journal on this route does get covered, the seam it needs should be
/// built against that test rather than ahead of it.
pub(crate) struct AcpPromptSink {
    transport: Arc<acp_client::AcpTransport>,
}

impl AcpPromptSink {
    pub(super) fn new(transport: &Arc<acp_client::AcpTransport>) -> Self {
        Self {
            transport: Arc::clone(transport),
        }
    }

    /// The delivery this prompt is authorised for, read live from the
    /// session's negotiated capability — not from a copy taken at spawn.
    /// A `session/load` handshake re-derives the verdict like the rest of
    /// the negotiated state, so a resumed session cannot send on a stale yes.
    pub(crate) fn delivery(&self) -> ImageDelivery {
        ImageDelivery::from_negotiated(self.transport.prompt_capabilities().image)
    }

    /// Sends one planned prompt as structured content: the plan's text block,
    /// then one image block per raster attachment, in attachment order.
    /// Takes the whole plan (not `fallback_text` + `images` separately) so
    /// the text block the child receives and the string the journal records
    /// are the same value by construction — a later edit cannot pass one
    /// string to the wire and journal another.
    pub(crate) fn send_structured_prompt(
        &self,
        plan: StructuredPromptPlan,
    ) -> Result<(), WireError> {
        // The capability is re-read here, at send time: only a negotiated
        // `Supported` takes this path. Anything else never reaches the sink
        // — the caller falls back to the path line instead.
        debug_assert!(matches!(
            self.delivery(),
            ImageDelivery::NegotiatedImageBlock
        ));
        self.deliver(plan)?;
        Ok(())
    }

    /// The delivery call: the plan's content blocks go to the child through
    /// the shared transport. Kept separate from
    /// [`Self::send_structured_prompt`] so the capability re-read and the
    /// write stay two readable steps rather than one.
    fn deliver(&self, plan: StructuredPromptPlan) -> Result<(), WireError> {
        self.transport
            .send_structured_prompt(plan.content_blocks())
            .map_err(|error| {
                WireError::new(
                    ErrorCode::Io,
                    format!("Could not send input to the terminal: {error}"),
                )
            })?;
        Ok(())
    }
}

/// The static counterpart of [`AcpPromptSink`]: the structured prompt route
/// for a provider whose protocol carries images but whose handshake says
/// nothing the daemon reads — Claude, Codex and Pi, the three that answer
/// [`ImageDelivery::StaticImageBlock`].
///
/// Planning and sending are two steps here for the same reason they are two
/// on the ACP route: the base64 decode, the container sniff and the strip
/// walk run before the writer is locked, and the frame goes out under that
/// hold, so the journal entry that follows keeps the order the child sees.
pub(crate) trait StaticImageSink: Send + Sync {
    /// Decides one prompt: materializes every attachment exactly once and
    /// answers with the plan — or `None` when this route does not run for the
    /// request, which is no attachments at all or a provider that is not
    /// authorised for inline bytes right now (a Pi model that declared no
    /// `image`, an unknown model, no current model). A `None` means nothing
    /// was materialized either, so the caller's legacy path-line write is the
    /// only walk of this request.
    ///
    /// The text travels inside the plan rather than beside it, so the string
    /// the frame carries and the string the journal records cannot be two
    /// different values, and so the caller never has to walk the attachments
    /// a second time through [`with_attachment_paths`].
    fn plan_prompt(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        text: &str,
        attachments: &[PromptAttachment],
    ) -> Result<Option<Box<dyn PlannedStaticPrompt>>, WireError>;
}

/// One static provider's planned prompt: the text it carries and whatever that
/// provider's own protocol sends beside it — Claude's `content[]` blocks,
/// Codex's `localImage` paths, Pi's `images[]` entries.
///
/// The blocks may be empty. A prompt whose attachments all take a path line
/// (an SVG, a gif whose container no walk follows) still travels as a plan,
/// and the frame it sends is the text-only frame byte for byte — which is what
/// keeps one send at one materialization per attachment, on every send.
pub(crate) trait PlannedStaticPrompt: Send + Sync {
    /// The text the frame carries — the same value the journal records.
    fn text(&self) -> &str;

    /// Appends one path line per resolved reference to the text this plan
    /// carries, after the path lines its own attachments left behind.
    ///
    /// A mutation rather than a parameter of
    /// [`StaticImageSink::plan_prompt`] because the plan owns its text on
    /// purpose: the frame this provider builds and the string the caller
    /// journals are one value, so the references have to be added to that
    /// value rather than to a copy beside it. The caller appends this before
    /// either of them reads the plan, and the route's own composition of the
    /// inline lines stays exactly where it is.
    ///
    /// A reference is a path line even for a provider on this list, which is
    /// the route that carries bytes inline — see
    /// [`push_reference_path_lines`] for why that is a decision.
    fn append_reference_path_lines(&mut self, reference_paths: &[PathBuf]);

    /// Frames and sends this prompt.
    fn send(&self) -> Result<(), WireError>;
}

/// The text block for a structured prompt: the user's text, a blank line,
/// then one path line per non-raster attachment. The same line shape
/// `with_attachment_paths` writes — both go through [`push_path_lines`], which
/// is where that line is written once — so the fallback reads identically
/// whether it travels alone or beside image blocks. `plan_structured_prompt`
/// is its only caller; it stays separate (rather than inlined) so the legacy
/// write and the structured text block visibly share one line shape.
pub(super) fn prompt_text_with_fallback_paths(text: &str, fallback_paths: &[PathBuf]) -> String {
    if fallback_paths.is_empty() {
        return text.to_string();
    }
    let mut prompt = String::from(text);
    prompt.push_str("\n\n");
    push_path_lines(&mut prompt, fallback_paths);
    prompt
}
