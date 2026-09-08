import type { AgentSessionState } from "../../lib/agentSession";
import type { ProviderInfo } from "../../types/ipc";

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
  prompt: string;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
  artifactHtml?: string;
  artifactError?: string;
  appliedSkillSlugs?: readonly string[];
  skillSelectionFallback?: boolean;
}

export interface DesignGenerationOptions {
  skills?: readonly string[];
  skillMode?: "auto";
}

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
  subscribeAgentSession?(listener: () => void): () => void;
  selectProvider?(provider: ProviderInfo): void;
}

export interface DesignAgentSession {
  getState(): AgentSessionState;
  subscribe(listener: () => void): () => void;
  setModel(modelId?: string, effort?: string): Promise<void>;
}
