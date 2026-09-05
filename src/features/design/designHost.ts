export type DesignTool = "move" | "ai";
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
}

export type DesignMessage = DesignUserMessage | DesignAssistantMessage;

export interface DesignGenerationResult {
  prompt: string;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
  artifactHtml?: string;
}

export interface DesignCanvasContent {
  aiRegion: {
    x: number;
    y: number;
    width: number;
    height: number;
    actionLabel: string;
  };
}

export interface DesignInitialState {
  // Initial values; zoom and tool seed the view but are not rewritten by document saves.
  tool: DesignTool;
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
  provider: string;
  contextPrefix: string;
  draftPlaceholder: string;
  noContextPlaceholder: string;
  tokenFooter: string;
  initialState: DesignInitialState;
  // Persisted document preferences, kept outside the undo snapshot.
  selectedLayerId: string;
  grounded: boolean;
  layers: readonly DesignLayer[];
  canvasContent: DesignCanvasContent;
  radiusOptions: readonly DesignRadiusOption[];
  messages: readonly DesignMessage[];
  workingMessage: Pick<DesignAssistantMessage, "title" | "desc">;
}

export interface DesignHost {
  loadDocument(): Promise<DesignDocument>;
  saveDocument?(doc: DesignDocument): Promise<void>;
  generate?(prompt: string, signal: AbortSignal): Promise<DesignGenerationResult>;
}
