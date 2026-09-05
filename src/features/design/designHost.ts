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
  id: number;
  role: "user";
  text: string;
  ctx?: string;
}

export interface DesignAssistantMessage {
  id: number;
  role: "assistant";
  status: DesignMessageStatus;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
  instruction?: string;
}

export type DesignMessage = DesignUserMessage | DesignAssistantMessage;

export interface DesignGenerationResult {
  prompt: string;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
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
  tool: DesignTool;
  zoom: number;
  radius: number;
  flat: boolean;
  saved: boolean;
  grounded: boolean;
  draft: string;
  selectedLayerId: string;
  hiddenLayerIds: readonly string[];
}

export interface DesignDocument {
  name: string;
  path: string;
  provider: string;
  contextPrefix: string;
  draftPlaceholder: string;
  noContextPlaceholder: string;
  generationFooter: string;
  tokenFooter: string;
  initialState: DesignInitialState;
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
