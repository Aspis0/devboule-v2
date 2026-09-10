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
export type DesignRadiusToken = "none" | "sm" | "md" | "lg";
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
   * Present only on SECTION layers: a measured landmark or heading of the
   * generated page. The transform is the artifact origin plus the measured
   * page rect (canvas world coordinates); `tag`/`anchor` identify the element
   * inside the page. No source file, no corners, no elevation: duplicating or
   * deleting a measured section is meaningless, so those actions stay hidden.
   */
  section?: { tag: string; anchor: string };
}

export interface DesignRadiusOption {
  token: DesignRadiusToken;
  value: number;
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
 */
export type DesignGenerationOptions = (
  | { skillMode: "auto" }
  | { skillMode: "all" }
  | { skillMode: "manual"; skills: readonly string[] }
) & { grounded?: boolean; folderPath?: string | null };

export interface DesignInitialState {
  // Initial values; zoom seeds the view but is not rewritten by document saves.
  zoom: number;
  radius: number;
  flat: boolean;
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
  tokenFooter: string;
  initialState: DesignInitialState;
  // Persisted document preferences, kept outside the undo snapshot.
  selectedLayerId: string;
  grounded: boolean;
  layers: readonly DesignLayer[];
  layerNotice?: string;
  /** Anchored agent notes, in creation order. Absent means none were ever added. */
  sectionNotes?: readonly SectionNote[];
  radiusOptions: readonly DesignRadiusOption[];
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
}
