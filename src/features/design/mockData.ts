/** M1b mock boundary. Replace this module with typed IPC adapters. */

import type {
  DesignDocument,
  DesignGenerationResult,
  DesignHost,
  DesignLayer,
  DesignMessage,
  DesignRadiusOption,
  DesignTool,
} from "./designHost";

export type {
  DesignAssistantMessage,
  DesignDocument,
  DesignGenerationResult,
  DesignHost,
  DesignLayer,
  DesignLayerKind,
  DesignMessage,
  DesignMessageStatus,
  DesignRadiusOption,
  DesignRadiusToken,
  DesignTool,
  DesignTransform,
  DesignUserMessage,
} from "./designHost";

export const MOCK_DESIGN_INITIAL_STATE = {
  tool: "move" as DesignTool,
  zoom: 1,
  radius: 14,
  flat: false,
  saved: false,
  grounded: true,
  draft: "",
  selectedLayerId: "index-header",
  hiddenLayerIds: [] as readonly string[],
};

export const MOCK_DESIGN_DOCUMENT = {
  name: "Index browser",
  path: "~/dev/devboule/src/design",
  provider: "Claude Code · High",
  contextPrefix: "Editing",
  draftPlaceholder: "Describe the change to Index header…",
  noContextPlaceholder: "Describe what to generate…",
  generationFooter:
    "Generations land on the Design canvas in this worktree; Save to repo writes them back as components.",
  tokenFooter: "Values snap to design tokens (DTCG)",
} as const;

export const MOCK_DESIGN_LAYERS: readonly DesignLayer[] = [
  {
    id: "index-header",
    name: "Index header",
    kind: "TSX",
    transform: { x: 396, y: 92, width: 336, height: 198, hug: true },
  },
  {
    id: "stale-queue",
    name: "Stale queue",
    kind: "TSX",
    transform: { x: 60, y: 46, width: 300, height: 124 },
  },
  {
    id: "reindex-bar",
    name: "Reindex bar",
    kind: "TSX",
    transform: { x: 60, y: 188, width: 280, height: 34 },
  },
  {
    id: "empty-state",
    name: "Empty state",
    kind: "SVG",
    transform: { x: 80, y: 252, width: 220, height: 120 },
  },
];

export const MOCK_DESIGN_CANVAS_CONTENT = {
  aiRegion: {
    x: 420,
    y: 300,
    width: 240,
    height: 96,
    actionLabel: "Analyze this region",
  },
} as const;

export const MOCK_DESIGN_RADIUS_OPTIONS: readonly DesignRadiusOption[] = [
  { token: "none", value: 0 },
  { token: "sm", value: 8 },
  { token: "md", value: 14 },
  { token: "lg", value: 22 },
];

export const MOCK_DESIGN_MESSAGES: readonly DesignMessage[] = [
  {
    id: 1,
    role: "user",
    text: "Make the header state the stale count and drop the second export button.",
    ctx: "Editing Index header",
  },
  {
    id: 2,
    role: "assistant",
    status: "done",
    title: "Edited Index header",
    desc: "Pulled the count from the real hygiene snapshot and removed the duplicate action. Radius and shadow snapped to radius.md / shadow.soft.",
    sources: ["WorkspaceView.tsx", "oracle-core/src/classify.rs", "tokens.json"],
    nodeIds: ["index-header"],
    instruction: "edit",
  },
];

export const MOCK_DESIGN_GENERATION_RESULTS = {
  edit: {
    prompt: "Apply the requested edit.",
    title: "Edited Index header",
    desc: "Applied to the selected node and committed to the manifest, so undo takes it back in one step. Values snapped to radius.md and shadow.soft.",
    sources: ["WorkspaceView.tsx", "tokens.json"],
    nodeIds: ["index-header"],
  },
  retry: {
    prompt: "Retry.",
    title: "Edited Index header",
    desc: "Second attempt succeeded — the node was written and the manifest committed.",
    sources: ["WorkspaceView.tsx"],
    nodeIds: ["index-header"],
  },
  regenerate: {
    prompt: "Regenerate that edit.",
    title: "Regenerated Index header",
    desc: "Same instruction, new pass. The count row is now a single line and the actions collapsed to one primary.",
    sources: ["WorkspaceView.tsx", "tokens.json"],
    nodeIds: ["index-header"],
  },
  visualCheck: {
    prompt: "Run a visual check on the canvas.",
    title: "Visual check passed",
    desc: "Captured the canvas and compared it against the tokens: contrast holds at 4.9:1, no node overlaps, and every radius resolves to a token. The stale count reads from the snapshot, not a literal.",
    sources: ["tokens.json", "index_writer.rs"],
    nodeIds: ["index-header", "stale-queue"],
  },
} as const satisfies Record<string, DesignGenerationResult>;

export const MOCK_DESIGN_WORKING_MESSAGE = {
  title: "Generating…",
  desc: "Reading the grounded files, then writing the node.",
} as const;

function cloneDesignDocument(document: DesignDocument): DesignDocument {
  return {
    ...document,
    initialState: {
      ...document.initialState,
      hiddenLayerIds: [...document.initialState.hiddenLayerIds],
    },
    layers: document.layers.map((layer) => ({
      ...layer,
      transform: { ...layer.transform },
    })),
    canvasContent: { aiRegion: { ...document.canvasContent.aiRegion } },
    radiusOptions: document.radiusOptions.map((option) => ({ ...option })),
    messages: document.messages.map((message) =>
      message.role === "user"
        ? { ...message }
        : { ...message, sources: [...message.sources], nodeIds: [...message.nodeIds] },
    ),
    workingMessage: { ...document.workingMessage },
  };
}

function createDemoDocument(): DesignDocument {
  return {
    ...MOCK_DESIGN_DOCUMENT,
    initialState: { ...MOCK_DESIGN_INITIAL_STATE },
    layers: MOCK_DESIGN_LAYERS,
    canvasContent: MOCK_DESIGN_CANVAS_CONTENT,
    radiusOptions: MOCK_DESIGN_RADIUS_OPTIONS,
    messages: MOCK_DESIGN_MESSAGES,
    workingMessage: MOCK_DESIGN_WORKING_MESSAGE,
  };
}

export function createDemoHost(): DesignHost {
  let currentDocument = createDemoDocument();

  return {
    loadDocument: async () => cloneDesignDocument(currentDocument),
    saveDocument: async (document) => {
      currentDocument = cloneDesignDocument(document);
    },
    generate: async (prompt, signal) => {
      await Promise.resolve();
      if (signal.aborted) throw new DOMException("Generation aborted", "AbortError");

      const normalizedPrompt = prompt.toLowerCase();
      const result = normalizedPrompt.includes("visual check")
        ? MOCK_DESIGN_GENERATION_RESULTS.visualCheck
        : normalizedPrompt.includes("retry")
          ? MOCK_DESIGN_GENERATION_RESULTS.retry
          : normalizedPrompt.includes("regenerate")
            ? MOCK_DESIGN_GENERATION_RESULTS.regenerate
            : MOCK_DESIGN_GENERATION_RESULTS.edit;
      return { ...result, prompt };
    },
  };
}
