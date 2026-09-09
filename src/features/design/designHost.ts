import type { AgentSessionState } from "../../lib/agentSession";
import type { ProviderInfo, Session, Workspace } from "../../types/ipc";

export type DesignLayerKind = "TSX" | "SVG";
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
}

export interface DesignRadiusOption {
  token: DesignRadiusToken;
  value: number;
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
 */
export type DesignGenerationOptions =
  | { skillMode: "auto" }
  | { skillMode: "all" }
  | { skillMode: "manual"; skills: readonly string[] };

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
  /** The daemon record for the live session, including its echoed working directory. */
  getAgentSessionRecord?(): Session | null;
  subscribeAgentSession?(listener: () => void): () => void;
  selectProvider?(provider: ProviderInfo): void;
  selectWorkspace?(workspace: Workspace | null): void;
}

export interface DesignAgentSession {
  getState(): AgentSessionState;
  subscribe(listener: () => void): () => void;
  setModel(modelId?: string, effort?: string): Promise<void>;
}
