import type { AgentSessionState } from "../../lib/agentSession";
import type { PermissionRequest, ProviderInfo, Session, Workspace } from "../../types/ipc";

/**
 * One line of the agent's own conversation: its prose, its reasoning, and its
 * tool activity, in arrival order. User messages are excluded on purpose. The
 * Design surface renders the prompt the user typed itself, and the daemon also
 * echoes that prompt back as `agent_user_message` chunks — the echo carries the
 * full grounded prompt, doctrine block included, so rendering it would show the
 * user a message they never wrote.
 *
 * The shape is stated here rather than derived from the session's own item type
 * because that type's text arm carries `role: "user" | "assistant" | "thought"`,
 * so a role-based narrowing would erase the prose and reasoning rows entirely.
 */
export type DesignTranscriptItem =
  | {
      id: string;
      role: "assistant" | "thought";
      text: string;
      messageId: string | null;
      parentToolUseId?: string;
      spawnDepth?: number;
    }
  | {
      id: string;
      role: "tool";
      text: string;
      toolCallId: string;
      status: string;
      parentToolUseId?: string;
      spawnDepth?: number;
      subagentType?: string;
    };

export type DesignLayerKind = "TSX" | "SVG" | "SECTION";
export type DesignMessageStatus = "working" | "done" | "error";

export interface DesignTransform {
  x: number;
  y: number;
  width: number;
  height: number;
  hug?: boolean;
}

export interface DesignLayer {
  id: string;
  name: string;
  kind: DesignLayerKind;
  transform: DesignTransform;
  source?: { path: string };
  /**
   * Present only on SECTION layers: a measured landmark, heading, text leaf,
   * control, or media element of the generated page. The transform is the
   * artifact origin plus the measured page rect (canvas world coordinates);
   * `tag`/`anchor` identify the element inside the page. No source file, no
   * corners, no elevation: duplicating or deleting a measured section is
   * meaningless, so those actions stay hidden.
   *
   * `parentId` is the id of the nearest collected ancestor inside the page, or
   * absent for a root-level element. It is the parent chain the panel walks to
   * rebuild the page tree from a clicked phrase up to the slide that holds it.
   * An id, not a position in the measured list: `displayLayers` puts canvas
   * layers ahead of the measured sections, so an index would desync the tree on
   * the first filter or reorder.
   */
  section?: { tag: string; anchor: string; parentId?: string };
}

/**
 * One anchored note for the agent, attached to a page section by its stable
 * anchor (the element id, or the computed tag path when the element has
 * none). Notes travel with the document exactly like messages do: the host
 * persists them on save, and the live session mirrors them in the store so
 * they survive surface navigation.
 */
export interface SectionNote {
  anchor: string;
  text: string;
}

export interface DesignUserMessage {
  id: string;
  role: "user";
  text: string;
  ctx?: string;
}

export interface DesignAssistantMessage {
  id: string;
  role: "assistant";
  status: DesignMessageStatus;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
  instruction?: string;
  artifactHtml?: string;
  artifactError?: string;
  /**
   * The output shape the run that produced this artifact declared
   * (`page` | `slides`), recorded at the moment the artifact was produced.
   * The surface reads the slides-shape notice off this, never off the live
   * output switch: the switch states what the next run will ask for and
   * regenerates nothing, so it cannot describe the artifact already on screen.
   *
   * Absent is a third state, not `page`: a message restored from a document
   * saved before this field existed, or one whose host did not report a mode,
   * was given no shape contract, and a contract nobody can show was applied is
   * nothing to report on.
   */
  outputMode?: DesignOutputMode;
  /**
   * How many non-empty ```html blocks the reply that produced this artifact
   * carried, recorded by the producing run. Last-wins is deliberate — a
   * correcting agent emits a second, better block — but a reply that carried
   * six blocks and put one on the canvas must not be silent about the other
   * five. The canvas reports it when it is more than one.
   *
   * Absent is a third state, not one block: a message restored from a document
   * saved before this field existed, or reopened from a history entry, which
   * records no count, has no fact to report and stays silent rather than
   * asserting a reply carried exactly one block.
   */
  fencedHtmlBlockCount?: number;
  /**
   * One quiet line about Oracle grounding for this run, shown under the
   * summary. Set only when a grounded run could not use the attached
   * folder's index; absent or null means nothing to report (grounded on the
   * folder, ungrounded by choice, or no folder attached).
   */
  groundingNotice?: string | null;
  /**
   * What the agent actually said and did for this generation, kept so the
   * conversation survives the run and later sessions. Absent means the host
   * never reported a transcript (a history entry, or a host with no agent).
   */
  transcript?: readonly DesignTranscriptItem[];
}

export type DesignMessage = DesignUserMessage | DesignAssistantMessage;

export interface DesignGenerationResult {
  /** Identity of the session that produced this result, not whichever session is current later. */
  sessionId: string;
  peerSessionId: string | null;
  /** Creation time of the session that produced this result, or null for non-session hosts. */
  createdAtMs: number | null;
  prompt: string;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
  artifactHtml?: string;
  artifactError?: string;
  /**
   * The output shape this run was asked to produce (`page` | `slides`), taken
   * verbatim from `DesignGenerationOptions.outputMode` so the surface can
   * record it on the artifact it produces. A request that named no mode leaves
   * it absent; see `DesignAssistantMessage.outputMode` for why absent is not
   * `page`. The host deliberately does not substitute its own `page` default
   * here, which is a behaviour for a run that predates slides, not a shape the
   * caller declared.
   */
  outputMode?: DesignOutputMode;
  /**
   * How many non-empty ```html blocks the reply carried, taken from the reply
   * the artifact was extracted from so the surface can report a selection that
   * dropped earlier blocks. Absent when the run produced no artifact, or when
   * the host did not report one; see
   * `DesignAssistantMessage.fencedHtmlBlockCount` for why absent is not one.
   */
  fencedHtmlBlockCount?: number;
  /** The run's conversation; see DesignAssistantMessage.transcript. */
  transcript?: readonly DesignTranscriptItem[];
  /**
   * The skill sections requested for this generation, in composition order,
   * before composed-budget truncation. That is the list's one meaning: its
   * length carries no signal, and whether the mode's own chooser decided is
   * reported by `skillSelectionFallback`, never by counting entries. Present
   * for every host that runs a skill-selection step; an empty array is a
   * real, reported selection of nothing (e.g. a manual pin of zero
   * sections). Absent means the host performed no skill selection at all,
   * not "unknown".
   */
  appliedSkillSlugs?: readonly string[];
  /**
   * True when the mode's own chooser could not decide and the default
   * priority order was used instead (a failed automatic pre-flight turn, or
   * a matched request with no strong section match). A user pin never falls
   * back, so a pinned generation is always false. This field is the only
   * authority on "did the chooser decide"; consumers must not infer it from
   * anything else.
   */
  skillSelectionFallback?: boolean;
  /**
   * One quiet line about Oracle grounding for this run. Set only when a
   * grounded run could not use the attached folder's index; absent or null
   * means nothing to report (grounded on the folder, ungrounded by choice,
   * or no folder attached). The surface renders it under the summary.
   */
  groundingNotice?: string | null;
}

/**
 * Declared output shape of a generation: a scrolling page or a slide deck.
 * Declared on the wire by the caller, never inferred from the prompt text:
 * inferring a mode from a derived value was tried for the skill mode and
 * rejected — it degraded silently whenever the derived value moved while
 * the UI kept saying something else. Absent means a caller that predates
 * slides and keeps the page behaviour; the surface always states it
 * explicitly, exactly like `grounded` and `folderPath`.
 */
export type DesignOutputMode = "page" | "slides";

/**
 * One file the user imported into the composer as a starting point. Two shapes,
 * because they travel as two different things.
 *
 * A raster image is bytes: it is carried base64, the form every provider that
 * accepts an image expects on the wire, matching the `{ data, mimeType }` pair
 * Paseo sends.
 *
 * An SVG is not an image here. It is a text document this surface can embed in
 * the HTML it generates, so it is carried as sanitized source. That is a
 * deliberate departure from Paseo, which classifies SVG as a generic file and
 * sends the agent four lines of metadata; this surface generates HTML, and an
 * SVG is something it can use directly. `source` has already been through the
 * sanitizer in `designAttachments.ts` and never travels raw.
 *
 * The measured type is the one stored, never the declared one: see the module
 * comment in `designAttachments.ts` for why the bytes decide.
 */
export interface DesignRasterAttachment {
  id: string;
  kind: "raster";
  name: string;
  mimeType: "image/png" | "image/jpeg";
  bytes: number;
  /** The file's bytes, base64, with no `data:` prefix. */
  base64: string;
  /** Present only on a picture that is a page of a document the user attached. */
  document?: DesignAttachmentDocument;
}

export interface DesignSvgAttachment {
  id: string;
  kind: "svg";
  name: string;
  mimeType: "image/svg+xml";
  /** UTF-8 length of `source`, not of the file the user picked. */
  bytes: number;
  /** Sanitized SVG source: XML prologue stripped, ready to embed in a page. */
  source: string;
}

export type DesignAttachment = DesignRasterAttachment | DesignSvgAttachment;

/**
 * The document a raster came from, when it did not come from a picture.
 *
 * A PDF is one attachment the user picked and N pictures the composer carries,
 * one per page, because the wire is per-image: a provider takes `{ data,
 * mimeType }` per image and the daemon stores what it is handed. That is a
 * transport fact, and these fields are the other half of it — the pictures know
 * which document they belong to, which is what lets the composer show one pill
 * for the file the user chose and take the whole document away in one action
 * rather than leaving a deck with a hole in it.
 *
 * Optional, and absent on a PNG, a JPEG or an SVG the user picked directly:
 * those are their own attachment, which is what every reader of this type
 * already assumes. Every page of one document carries the same `id`, generated
 * once per import rather than derived from the file name, so two documents that
 * happen to share a name stay two documents.
 */
export interface DesignAttachmentDocument {
  /** Generated once per import; every page of the document carries this value. */
  id: string;
  /** What the user called it: the name of the file they picked. */
  name: string;
  /** This picture's page, 1-based. */
  page: number;
  /** Pages the document has. */
  pageCount: number;
  /**
   * Pages that travelled: `pageCount` when the whole document fit, fewer when
   * the composer's budget cut it short. The import's notice names the pages that
   * stayed behind and which cause lost them.
   */
  travelled: number;
}

/**
 * Call-time options, never persisted. The skill mode is declared on the wire
 * with the same ids the persisted selection uses (`all` | `manual` | `auto`):
 * the caller knows which mode is active and says so; the host never has to
 * infer intention from the shape of the list. Inference from shape was tried
 * and rejected — a list-coverage heuristic died silently whenever a derived
 * constant moved, misreported a reordered list as a pin, and made callers
 * encode meaning in array length.
 *
 * - `{ skillMode: "all" }` — Matched: the host ranks the built-in corpus
 *   against the request text with the deterministic lexical ranker, no model
 *   turn. No list travels: the corpus is the host's to index.
 * - `{ skillMode: "manual", skills }` — exactly the listed sections,
 *   composed verbatim in the given order.
 * - `{ skillMode: "auto" }` — a pre-flight agent turn picks the sections;
 *   the only mode that costs an extra model turn.
 *
 * `grounded` says whether the run may search the repository through Oracle
 * before composing its prompt. It is absent when a caller predates the toggle,
 * and the host then keeps the grounded behaviour. The surface always states it
 * explicitly, so the toolbar's grounding control is the only thing deciding it.
 *
 * `folderPath` is the absolute folder the canvas is attached to, or null when
 * nothing is attached. It is absent when a caller predates folder-aware
 * grounding, and the host then keeps the legacy global-index behaviour. The
 * surface always states it explicitly: a string grounds the run on that
 * folder's own index, null means no grounding without a notice.
 *
 * `outputMode` names which shape the run must produce (`page` | `slides`).
 * It is absent when a caller predates slides, and the host then keeps the
 * page behaviour. The surface always states it explicitly, so the output
 * switch — which lives beside the surface until its file is free — is the
 * only thing deciding it.
 *
 * `attachments` are the files the user imported into the composer for this run,
 * in the order they appear above the text. They are call-time like every other
 * field here and are never persisted: a starting point belongs to the request it
 * was attached to, and the composer clears them the moment the run starts. The
 * field is optional and its absence means the caller predates importing, which
 * is the same thing as an empty list to every consumer that has one. The types
 * are declared here so the boundary is stated where the request is built rather
 * than inferred later from a prompt that happens to contain base64.
 */
export type DesignGenerationOptions = (
  | { skillMode: "auto" }
  | { skillMode: "all" }
  | { skillMode: "manual"; skills: readonly string[] }
) & {
  grounded?: boolean;
  folderPath?: string | null;
  outputMode?: DesignOutputMode;
  attachments?: readonly DesignAttachment[];
};

export interface DesignInitialState {
  // Initial values; zoom seeds the view but is not rewritten by document saves.
  zoom: number;
  saved: boolean;
  draft: string;
  hiddenLayerIds: readonly string[];
}

export interface DesignDocument {
  name: string;
  path: string;
  contextPrefix: string;
  draftPlaceholder: string;
  noContextPlaceholder: string;
  initialState: DesignInitialState;
  // Persisted document preferences, kept outside the undo snapshot.
  selectedLayerId: string;
  grounded: boolean;
  layers: readonly DesignLayer[];
  layerNotice?: string;
  /** Anchored agent notes, in creation order. Absent means none were ever added. */
  sectionNotes?: readonly SectionNote[];
  messages: readonly DesignMessage[];
  workingMessage: Pick<DesignAssistantMessage, "title" | "desc">;
}

export interface DesignHost {
  loadDocument(signal?: AbortSignal): Promise<DesignDocument>;
  saveDocument?(doc: DesignDocument): Promise<void>;
  generate?(
    prompt: string,
    signal: AbortSignal,
    options?: DesignGenerationOptions,
  ): Promise<DesignGenerationResult>;
  /** Optional live session capability supplied by the agent-backed host. */
  getAgentSession?(): DesignAgentSession | null;
  /**
   * Index into `getAgentSession()?.getState().items` at which the latest run's
   * conversation begins, or null before any run has recorded its boundary (which
   * includes the craft-selection pre-flight that precedes the first run). The host
   * owns this boundary because that pre-flight is the host's own question: its items
   * must not be presented as the agent's answer to the user. The surface slices the
   * live items from here to stream the conversation while the agent works, and the
   * boundary outlives the run so the working card does not go blank in the instant
   * before the finished transcript arrives on the result.
   */
  getRunTranscriptStart?(): number | null;
  /** The first queued permission currently holding an agent turn open, if any. */
  getPendingPermission?(): PendingPermission | null;
  /** Short-lived notice for a permission resolved without a Design answer (for example timeout). */
  getPermissionNotice?(): string | null;
  /** Answer the Design surface's currently displayed permission request. */
  respondPermission?(outcome: "allow_once" | "deny"): Promise<void>;
  /** The daemon record for the live session, including its echoed working directory. */
  getAgentSessionRecord?(): Session | null;
  subscribeAgentSession?(listener: () => void): () => void;
  /** Commit a provider preference and close any current session without opening a replacement. */
  setProviderPreference?(provider: ProviderInfo): void;
  /** Commit a workspace preference and close any current session without opening a replacement. */
  setWorkspacePreference?(workspace: Workspace | null): void;
  /** End the current session and release its daemon resources, if one is live. */
  closeAgentSession?(): Promise<void>;
  selectProvider?(provider: ProviderInfo): void;
  selectWorkspace?(workspace: Workspace | null): void;
}

export interface PendingPermission {
  sessionId: string;
  subscriptionId: number;
  request: PermissionRequest;
}

export interface DesignAgentSession {
  getState(): AgentSessionState;
  subscribe(listener: () => void): () => void;
  setModel(modelId?: string, effort?: string): Promise<void>;
  setMode(modeId: string): Promise<void>;
}
