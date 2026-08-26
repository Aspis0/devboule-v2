/** M1b mock boundary. Replace this module with typed IPC adapters. */

export type DesignTool = 'move' | 'ai';
export type DesignLayerKind = 'TSX' | 'SVG';
export type DesignRadiusToken = 'none' | 'sm' | 'md' | 'lg';
export type DesignMessageStatus = 'working' | 'done' | 'error';

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
  role: 'user';
  text: string;
  ctx?: string;
}

export interface DesignAssistantMessage {
  id: number;
  role: 'assistant';
  status: DesignMessageStatus;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
  instruction?: string;
}

export type DesignMessage = DesignUserMessage | DesignAssistantMessage;

export type DesignCanvasNodeVariant = 'stale-queue' | 'index-header';

export interface DesignCanvasNode {
  id: string;
  variant: DesignCanvasNodeVariant;
  name: string;
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface DesignGenerationResult {
  prompt: string;
  title: string;
  desc: string;
  sources: readonly string[];
  nodeIds: readonly string[];
}

export const MOCK_DESIGN_INITIAL_STATE = {
  tool: 'move' as DesignTool,
  zoom: 1,
  radius: 14,
  flat: false,
  saved: false,
  grounded: true,
  draft: '',
  selectedLayerId: 'index-header',
  hiddenLayerIds: [] as readonly string[],
};

export const MOCK_DESIGN_DOCUMENT = {
  name: 'Index browser',
  path: '~/dev/devboule/src/design',
  provider: 'Claude Code · High',
  contextPrefix: 'Editing',
  draftPlaceholder: 'Describe the change to Index header…',
  noContextPlaceholder: 'Describe what to generate…',
  generationFooter: 'Generations land on the Design canvas in this worktree; Save to repo writes them back as components.',
  tokenFooter: 'Values snap to design tokens (DTCG)',
} as const;

export const MOCK_DESIGN_LAYERS: readonly DesignLayer[] = [
  {
    id: 'index-header',
    name: 'Index header',
    kind: 'TSX',
    transform: { x: 396, y: 92, width: 336, height: 198, hug: true },
  },
  {
    id: 'stale-queue',
    name: 'Stale queue',
    kind: 'TSX',
    transform: { x: 60, y: 46, width: 300, height: 124 },
  },
  {
    id: 'reindex-bar',
    name: 'Reindex bar',
    kind: 'TSX',
    transform: { x: 60, y: 188, width: 280, height: 34 },
  },
  {
    id: 'empty-state',
    name: 'Empty state',
    kind: 'SVG',
    transform: { x: 80, y: 252, width: 220, height: 120 },
  },
];

export const MOCK_DESIGN_CANVAS_NODES: readonly DesignCanvasNode[] = [
  {
    id: 'stale-queue',
    variant: 'stale-queue',
    name: 'Stale queue',
    x: 60,
    y: 46,
    width: 300,
    height: 124,
  },
  {
    id: 'index-header',
    variant: 'index-header',
    name: 'Index header',
    x: 396,
    y: 92,
    width: 336,
    height: 198,
  },
];

export const MOCK_DESIGN_CANVAS_CONTENT = {
  staleQueue: {
    label: 'Stale queue',
    rowWidths: [100, 82, 68],
  },
  indexHeader: {
    label: 'Index header',
    staleBadge: '37 stale',
    cardCount: 3,
    selectedCardIndex: 2,
    primaryAction: 'Reindex',
    secondaryAction: 'Export',
  },
  aiRegion: {
    x: 420,
    y: 300,
    width: 240,
    height: 96,
    actionLabel: 'Analyze this region',
  },
} as const;

export const MOCK_DESIGN_RADIUS_OPTIONS: readonly DesignRadiusOption[] = [
  { token: 'none', value: 0 },
  { token: 'sm', value: 8 },
  { token: 'md', value: 14 },
  { token: 'lg', value: 22 },
];

export const MOCK_DESIGN_MESSAGES: readonly DesignMessage[] = [
  {
    id: 1,
    role: 'user',
    text: 'Make the header state the stale count and drop the second export button.',
    ctx: 'Editing Index header',
  },
  {
    id: 2,
    role: 'assistant',
    status: 'done',
    title: 'Edited Index header',
    desc: 'Pulled the count from the real hygiene snapshot and removed the duplicate action. Radius and shadow snapped to radius.md / shadow.soft.',
    sources: ['WorkspaceView.tsx', 'oracle-core/src/classify.rs', 'tokens.json'],
    nodeIds: ['index-header'],
    instruction: 'edit',
  },
];

export const MOCK_DESIGN_GENERATION_RESULTS = {
  edit: {
    prompt: 'Apply the requested edit.',
    title: 'Edited Index header',
    desc: 'Applied to the selected node and committed to the manifest, so undo takes it back in one step. Values snapped to radius.md and shadow.soft.',
    sources: ['WorkspaceView.tsx', 'tokens.json'],
    nodeIds: ['index-header'],
  },
  retry: {
    prompt: 'Retry.',
    title: 'Edited Index header',
    desc: 'Second attempt succeeded — the node was written and the manifest committed.',
    sources: ['WorkspaceView.tsx'],
    nodeIds: ['index-header'],
  },
  regenerate: {
    prompt: 'Regenerate that edit.',
    title: 'Regenerated Index header',
    desc: 'Same instruction, new pass. The count row is now a single line and the actions collapsed to one primary.',
    sources: ['WorkspaceView.tsx', 'tokens.json'],
    nodeIds: ['index-header'],
  },
  visualCheck: {
    prompt: 'Run a visual check on the canvas.',
    title: 'Visual check passed',
    desc: 'Captured the canvas and compared it against the tokens: contrast holds at 4.9:1, no node overlaps, and every radius resolves to a token. The stale count reads from the snapshot, not a literal.',
    sources: ['tokens.json', 'index_writer.rs'],
    nodeIds: ['index-header', 'stale-queue'],
  },
} as const satisfies Record<string, DesignGenerationResult>;

export const MOCK_DESIGN_WORKING_MESSAGE = {
  title: 'Generating…',
  desc: 'Reading the grounded files, then writing the node.',
} as const;
