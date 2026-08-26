import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChangeEvent, KeyboardEvent, RefObject } from 'react';
import {
  MOCK_DESIGN_CANVAS_CONTENT,
  MOCK_DESIGN_CANVAS_NODES,
  MOCK_DESIGN_DOCUMENT,
  MOCK_DESIGN_GENERATION_RESULTS,
  MOCK_DESIGN_INITIAL_STATE,
  MOCK_DESIGN_LAYERS,
  MOCK_DESIGN_MESSAGES,
  MOCK_DESIGN_RADIUS_OPTIONS,
  MOCK_DESIGN_WORKING_MESSAGE,
  type DesignAssistantMessage,
  type DesignCanvasNode,
  type DesignGenerationResult,
  type DesignLayer,
  type DesignMessage,
  type DesignRadiusOption,
  type DesignTool,
} from './mockData';
import './design.css';

type MessageAction = 'stop' | 'retry' | 'select' | 'regenerate';

interface DesignSnapshot {
  hiddenLayerIds: readonly string[];
  radius: number;
  flat: boolean;
}

interface DesignViewState {
  selectedLayerId: string;
  tool: DesignTool;
  zoom: number;
}

interface DesignHistory {
  present: DesignSnapshot;
  past: DesignSnapshot[];
  future: DesignSnapshot[];
  saved: boolean;
}

interface LayerViewModel extends DesignLayer {
  selected: boolean;
  hidden: boolean;
}

interface RadiusViewModel extends DesignRadiusOption {
  selected: boolean;
}

interface MessageViewModel {
  message: DesignMessage;
  icon: string;
  iconTone: 'working' | 'error' | 'done';
  actions: readonly MessageAction[];
}

interface DesignToolbarProps {
  grounded: boolean;
  saved: boolean;
  busy: boolean;
  canUndo: boolean;
  canRedo: boolean;
  onGroundingToggle: () => void;
  onSave: () => void;
  onUndo: () => void;
  onRedo: () => void;
}

interface LayerPanelProps {
  layers: readonly LayerViewModel[];
  onSelect: (layerId: string) => void;
  onToggleVisibility: (layerId: string) => void;
}

interface CanvasProps {
  hiddenLayerIds: readonly string[];
  selectedLayerId: string;
  tool: DesignTool;
  zoom: number;
  onSelectLayer: (layerId: string) => void;
}

interface CanvasNodeProps {
  node: DesignCanvasNode;
  hidden: boolean;
  selected: boolean;
  onSelect: (layerId: string) => void;
}

interface ZoomControlsProps {
  zoom: number;
  canZoomIn: boolean;
  canZoomOut: boolean;
  onZoomIn: () => void;
  onZoomOut: () => void;
  onZoomReset: () => void;
  onFit: () => void;
}

interface InspectorProps {
  layer: DesignLayer;
  radiusOptions: readonly RadiusViewModel[];
  flat: boolean;
  onRadiusChange: (radius: number) => void;
  onElevationChange: (flat: boolean) => void;
}

interface AssistantProps {
  generationLabel: string;
  contextLayerName: string | null;
  provider: string;
  draft: string;
  draftPlaceholder: string;
  sendLabel: string;
  busy: boolean;
  messages: readonly MessageViewModel[];
  assistantRef: RefObject<HTMLDivElement>;
  onDraftChange: (event: ChangeEvent<HTMLTextAreaElement>) => void;
  onComposerKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  onSend: () => void;
  onVisualCheck: () => void;
  onClearContext: () => void;
  onMessageAction: (action: MessageAction, message: DesignMessage) => void;
}

const INITIAL_DOCUMENT_SNAPSHOT: DesignSnapshot = {
  hiddenLayerIds: MOCK_DESIGN_INITIAL_STATE.hiddenLayerIds,
  radius: MOCK_DESIGN_INITIAL_STATE.radius,
  flat: MOCK_DESIGN_INITIAL_STATE.flat,
};

const INITIAL_VIEW_STATE: DesignViewState = {
  selectedLayerId: MOCK_DESIGN_INITIAL_STATE.selectedLayerId,
  tool: MOCK_DESIGN_INITIAL_STATE.tool,
  zoom: MOCK_DESIGN_INITIAL_STATE.zoom,
};

const INITIAL_HISTORY: DesignHistory = {
  present: INITIAL_DOCUMENT_SNAPSHOT,
  past: [],
  future: [],
  saved: MOCK_DESIGN_INITIAL_STATE.saved,
};

type SnapshotChange = (current: DesignSnapshot) => DesignSnapshot | null;

function cloneMessages(): DesignMessage[] {
  return MOCK_DESIGN_MESSAGES.map((message) => {
    if (message.role === 'user') return { ...message };
    return {
      ...message,
      sources: [...message.sources],
      nodeIds: [...message.nodeIds],
    };
  });
}

function isHidden(hiddenLayerIds: readonly string[], layerId: string): boolean {
  return hiddenLayerIds.includes(layerId);
}

const DesignToolbar = memo(function DesignToolbar({
  grounded,
  saved,
  busy,
  canUndo,
  canRedo,
  onGroundingToggle,
  onSave,
  onUndo,
  onRedo,
}: DesignToolbarProps) {
  const saveText = busy ? 'Saving…' : saved ? 'Saved' : 'Unsaved changes';

  return (
    <header className="design-toolbar">
      <button className="design-browser-selector" type="button" aria-label="Choose design document">
        <span className="design-toolbar-dot" aria-hidden="true" />
        <span className="design-browser-name">{MOCK_DESIGN_DOCUMENT.name}</span>
        <span className="design-chevron" aria-hidden="true">▾</span>
      </button>
      <span className="design-path" title={MOCK_DESIGN_DOCUMENT.path}>{MOCK_DESIGN_DOCUMENT.path}</span>
      <span className="design-save-status" aria-live="polite">
        <span className={`design-save-dot${saved && !busy ? ' design-save-dot-saved' : ''}`} aria-hidden="true" />
        {saveText}
      </span>
      <span className="design-toolbar-spacer" />
      <span className="design-history-controls" aria-label="History controls">
        <button
          className="design-history-button"
          type="button"
          title="Undo (Ctrl+Z)"
          aria-label="Undo"
          onClick={onUndo}
          disabled={!canUndo}
        >
          ↶
        </button>
        <button
          className="design-history-button"
          type="button"
          title="Redo (Ctrl+Shift+Z)"
          aria-label="Redo"
          onClick={onRedo}
          disabled={!canRedo}
        >
          ↷
        </button>
      </span>
      <button
        className="design-grounding-toggle"
        type="button"
        title="Oracle grounding"
        aria-pressed={grounded}
        onClick={onGroundingToggle}
      >
        <span className={`design-grounding-dot${grounded ? ' design-grounding-dot-on' : ''}`} aria-hidden="true" />
        {grounded ? 'Grounded · devboule' : 'Not grounded'}
        <span className="design-chevron" aria-hidden="true">▾</span>
      </button>
      <button className="design-toolbar-button" type="button">Export</button>
      <button className="design-toolbar-button" type="button">Preview</button>
      <span className="design-save-actions">
        <button className="design-save-primary" type="button" onClick={onSave}>Save to repo</button>
        <button className="design-save-menu" type="button" title="More save options" aria-label="More save options">▾</button>
      </span>
    </header>
  );
});

const LayerPanel = memo(function LayerPanel({ layers, onSelect, onToggleVisibility }: LayerPanelProps) {
  return (
    <section className="design-layers-panel" aria-labelledby="design-layers-title">
      <div className="design-overlay-heading">
        <span id="design-layers-title">Layers</span>
        <span className="design-layer-count">{layers.length}</span>
        <span className="design-chevron" aria-hidden="true">▾</span>
      </div>
      <div className="design-layer-list">
        {layers.map((layer) => (
          <div className={`design-layer-row${layer.selected ? ' design-layer-row-selected' : ''}`} key={layer.id}>
            <button
              className="design-layer-select"
              type="button"
              aria-pressed={layer.selected}
              aria-label={`Select ${layer.name}`}
              onClick={() => onSelect(layer.id)}
            >
              <span className="design-layer-kind">{layer.kind}</span>
              <span className={`design-layer-name${layer.hidden ? ' design-layer-name-hidden' : ''}`}>{layer.name}</span>
            </button>
            <button
              className="design-layer-visibility"
              type="button"
              aria-pressed={!layer.hidden}
              aria-label={`${layer.hidden ? 'Show' : 'Hide'} ${layer.name}`}
              title="Hide / show"
              onClick={() => onToggleVisibility(layer.id)}
            >
              {layer.hidden ? '◌' : '◉'}
            </button>
          </div>
        ))}
      </div>
    </section>
  );
});

const CanvasNode = memo(function CanvasNode({ node, hidden, selected, onSelect }: CanvasNodeProps) {
  const content = node.variant === 'stale-queue' ? (
    <>
      <div className="design-node-heading">
        <span className="design-node-mark design-node-mark-terracotta" aria-hidden="true" />
        <span className="design-node-title">{MOCK_DESIGN_CANVAS_CONTENT.staleQueue.label}</span>
      </div>
      <div className="design-node-placeholder-list">
        {MOCK_DESIGN_CANVAS_CONTENT.staleQueue.rowWidths.map((width) => (
          <div className="design-node-placeholder" style={{ width: `${width}%` }} key={width} />
        ))}
      </div>
    </>
  ) : (
    <>
      <div className="design-node-heading">
        <span className="design-node-mark design-node-mark-purple" aria-hidden="true" />
        <span className="design-node-title">{MOCK_DESIGN_CANVAS_CONTENT.indexHeader.label}</span>
        <span className="design-node-badge">{MOCK_DESIGN_CANVAS_CONTENT.indexHeader.staleBadge}</span>
      </div>
      <div className="design-header-cards">
        {Array.from({ length: MOCK_DESIGN_CANVAS_CONTENT.indexHeader.cardCount }, (_, index) => (
          <div className={`design-header-card${index === MOCK_DESIGN_CANVAS_CONTENT.indexHeader.selectedCardIndex ? ' design-header-card-selected' : ''}`} key={index} />
        ))}
      </div>
      <div className="design-node-actions">
        <span className="design-node-primary-action">{MOCK_DESIGN_CANVAS_CONTENT.indexHeader.primaryAction}</span>
        <span className="design-node-secondary-action">{MOCK_DESIGN_CANVAS_CONTENT.indexHeader.secondaryAction}</span>
      </div>
      {selected ? (
        <>
          <span className="design-selection-handle design-selection-handle-tl" aria-hidden="true" />
          <span className="design-selection-handle design-selection-handle-tr" aria-hidden="true" />
          <span className="design-selection-handle design-selection-handle-bl" aria-hidden="true" />
          <span className="design-selection-handle design-selection-handle-br" aria-hidden="true" />
        </>
      ) : null}
    </>
  );

  return (
    <button
      className={`design-canvas-node design-canvas-node-${node.variant}${selected ? ' design-canvas-node-selected' : ''}${hidden ? ' design-canvas-node-hidden' : ''}`}
      type="button"
      style={{ left: node.x, top: node.y, width: node.width, minHeight: node.height }}
      aria-label={`Select ${node.name}`}
      aria-pressed={selected}
      disabled={hidden}
      onClick={() => onSelect(node.id)}
    >
      {content}
    </button>
  );
});

const ZoomControls = memo(function ZoomControls({ zoom, canZoomIn, canZoomOut, onZoomIn, onZoomOut, onZoomReset, onFit }: ZoomControlsProps) {
  const zoomLabel = `${Math.round(zoom * 100)}%`;

  return (
    <div className="design-zoom-controls" aria-label="Canvas zoom">
      <button type="button" title="Zoom out" aria-label="Zoom out" onClick={onZoomOut} disabled={!canZoomOut}>−</button>
      <button className="design-zoom-value" type="button" title="Reset to 100%" aria-label="Reset zoom to 100%" onClick={onZoomReset}>{zoomLabel}</button>
      <button type="button" title="Zoom in" aria-label="Zoom in" onClick={onZoomIn} disabled={!canZoomIn}>+</button>
      <button className="design-fit-button" type="button" title="Fit canvas" onClick={onFit}>Fit</button>
    </div>
  );
});

const DesignCanvas = memo(function DesignCanvas({ hiddenLayerIds, selectedLayerId, tool, zoom, onSelectLayer }: CanvasProps) {
  const aiRegion = MOCK_DESIGN_CANVAS_CONTENT.aiRegion;

  return (
    <div className="design-canvas" aria-label="Design canvas">
      <div className="design-canvas-grid" aria-hidden="true" />
      <div className="design-canvas-stage" style={{ transform: `scale(${zoom})` }}>
        {MOCK_DESIGN_CANVAS_NODES.map((node) => (
          <CanvasNode
            key={node.id}
            node={node}
            hidden={isHidden(hiddenLayerIds, node.id)}
            selected={selectedLayerId === node.id}
            onSelect={onSelectLayer}
          />
        ))}
        {tool === 'ai' ? (
          <div
            className="design-ai-region"
            style={{ left: aiRegion.x, top: aiRegion.y, width: aiRegion.width, height: aiRegion.height }}
          >
            <button type="button">{aiRegion.actionLabel}</button>
          </div>
        ) : null}
      </div>
    </div>
  );
});

const InspectorPanel = memo(function InspectorPanel({ layer, radiusOptions, flat, onRadiusChange, onElevationChange }: InspectorProps) {
  const transformFields = [
    ['X', layer.transform.x],
    ['Y', layer.transform.y],
    ['W', layer.transform.width],
    ['H', layer.transform.height],
  ] as const;

  return (
    <section className="design-inspector-panel" aria-labelledby="design-inspector-title">
      <div className="design-inspector-heading">
        <span id="design-inspector-title" className="design-inspector-name">{layer.name}</span>
        <span className="design-inspector-kind">{layer.kind}</span>
        <span className="design-inspector-close" aria-hidden="true">✕</span>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label">Transform</div>
        <div className="design-transform-grid">
          {transformFields.map(([label, value]) => (
            <span className="design-transform-field" key={label}>
              <span className="design-field-label">{label}</span>
              <span className="design-mono-value">{value}</span>
              {label === 'H' && layer.transform.hug ? <span className="design-hug-label">HUG</span> : null}
            </span>
          ))}
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label-row">
          <span className="design-inspector-label">Corners</span>
          <span className="design-token-value">radius.{radiusOptions.find((option) => option.selected)?.token ?? 'custom'}</span>
        </div>
        <div className="design-radius-picker">
          {radiusOptions.map((option) => (
            <button
              className={`design-radius-option${option.selected ? ' design-radius-option-selected' : ''}`}
              type="button"
              key={option.token}
              title={`radius.${option.token} · ${option.value}px`}
              aria-label={`Set radius.${option.token}, ${option.value} pixels`}
              aria-pressed={option.selected}
              onClick={() => onRadiusChange(option.value)}
            >
              <span className="design-radius-glyph" style={{ borderTopLeftRadius: `${Math.min(option.value, 12)}px` }} aria-hidden="true" />
            </button>
          ))}
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label-row">
          <span className="design-inspector-label">Elevation</span>
          <span className="design-token-value">{flat ? 'shadow.none' : 'shadow.soft'}</span>
        </div>
        <div className="design-segmented-control">
          <button type="button" aria-pressed={!flat} className={!flat ? 'design-segment-selected' : ''} onClick={() => onElevationChange(false)}>Soft</button>
          <button type="button" aria-pressed={flat} className={flat ? 'design-segment-selected' : ''} onClick={() => onElevationChange(true)}>Flat</button>
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label">Arrange</div>
        <div className="design-arrange-actions">
          <button type="button" title="Send to back" aria-label="Send layer to back">⤓</button>
          <button type="button" title="Move backward" aria-label="Move layer backward">↓</button>
          <button type="button" title="Move forward" aria-label="Move layer forward">↑</button>
          <button type="button" title="Bring to front" aria-label="Bring layer to front">⤒</button>
        </div>
      </div>

      <div className="design-inspector-actions">
        <button type="button">Duplicate</button>
        <button type="button" className="design-delete-action" aria-label="Delete layer">Delete</button>
      </div>
      <div className="design-inspector-footer">{MOCK_DESIGN_DOCUMENT.tokenFooter}</div>
    </section>
  );
});

const DesignMessageCard = memo(function DesignMessageCard({
  view,
  onAction,
}: {
  view: MessageViewModel;
  onAction: (action: MessageAction, message: DesignMessage) => void;
}) {
  const { message } = view;

  if (message.role === 'user') {
    return (
      <div className="design-message design-user-message-wrap">
        {message.ctx ? <div className="design-message-context">{message.ctx}</div> : null}
        <div className="design-user-message">{message.text}</div>
      </div>
    );
  }

  return (
    <div className="design-message-card">
      <div className="design-message-card-heading">
        <span className={`design-message-icon design-message-icon-${view.iconTone}`} aria-hidden="true">{view.icon}</span>
        <span className="design-message-title">{message.title}</span>
      </div>
      <div className="design-message-description">{message.desc}</div>
      {message.sources.length > 0 ? (
        <div className="design-message-sources">
          {message.sources.map((source) => <span key={source}>{source}</span>)}
        </div>
      ) : null}
      <div className="design-message-actions">
        {view.actions.map((action) => (
          <button type="button" key={action} onClick={() => onAction(action, message)}>
            {action === 'stop' ? 'Stop' : action === 'retry' ? 'Retry' : action === 'select' ? 'Select on canvas' : 'Regenerate'}
          </button>
        ))}
      </div>
    </div>
  );
});

const DesignAssistant = memo(function DesignAssistant({
  generationLabel,
  contextLayerName,
  provider,
  draft,
  draftPlaceholder,
  sendLabel,
  busy,
  messages,
  assistantRef,
  onDraftChange,
  onComposerKeyDown,
  onSend,
  onVisualCheck,
  onClearContext,
  onMessageAction,
}: AssistantProps) {
  return (
    <aside className="design-assistant" aria-labelledby="design-assistant-title">
      <div className="design-assistant-header">
        <span className="design-assistant-mark" aria-hidden="true" />
        <span id="design-assistant-title" className="design-assistant-title">Assistant</span>
        <span className="design-generation-label">{generationLabel}</span>
        <button className="design-visual-check" type="button" title="Visual check" aria-label="Run visual check" onClick={onVisualCheck}>◉</button>
      </div>

      <div className="design-assistant-scroll design-scroll" ref={assistantRef}>
        {messages.map((view) => <DesignMessageCard key={view.message.id} view={view} onAction={onMessageAction} />)}
      </div>

      <div className="design-composer-wrap">
        <div className="design-composer">
          {contextLayerName ? (
            <div className="design-composer-context">
              <span>{MOCK_DESIGN_DOCUMENT.contextPrefix} {contextLayerName}</span>
              <button type="button" title="Clear context" aria-label="Clear editing context" onClick={onClearContext}>✕</button>
            </div>
          ) : null}
          <textarea
            value={draft}
            onChange={onDraftChange}
            onKeyDown={onComposerKeyDown}
            placeholder={draftPlaceholder}
            aria-label="Describe a design change"
            rows={2}
          />
          <div className="design-composer-footer">
            <button className="design-provider-button" type="button" aria-label={`Model: ${provider}`}>
              <span className="design-provider-dot" aria-hidden="true" />
              {provider} ▾
            </button>
            <button className="design-generate-button" type="button" onClick={onSend} disabled={busy || !draft.trim()}>{sendLabel}</button>
          </div>
        </div>
        <div className="design-composer-hint"><b>Enter</b> to send · <b>Shift+Enter</b> for a new line</div>
      </div>
    </aside>
  );
});

export function DesignSurface() {
  const [history, setHistory] = useState<DesignHistory>(INITIAL_HISTORY);
  const [viewState, setViewState] = useState<DesignViewState>(INITIAL_VIEW_STATE);
  const [composerContextLayerId, setComposerContextLayerId] = useState<string | null>(INITIAL_VIEW_STATE.selectedLayerId);
  const [grounded, setGrounded] = useState(MOCK_DESIGN_INITIAL_STATE.grounded);
  const [draft, setDraft] = useState(MOCK_DESIGN_INITIAL_STATE.draft);
  const [busy, setBusy] = useState(false);
  const [messages, setMessages] = useState<DesignMessage[]>(cloneMessages);

  const generationTimerRef = useRef<number | null>(null);
  const assistantRef = useRef<HTMLDivElement>(null);
  const designSurfaceRef = useRef<HTMLElement>(null);

  const snapshot = history.present;
  const saved = history.saved;
  const { selectedLayerId, tool, zoom } = viewState;

  useEffect(() => {
    return () => {
      if (generationTimerRef.current !== null) window.clearTimeout(generationTimerRef.current);
    };
  }, []);

  useEffect(() => {
    if (assistantRef.current) assistantRef.current.scrollTop = assistantRef.current.scrollHeight;
  }, [messages, busy]);

  const selectedLayer = useMemo(
    () => MOCK_DESIGN_LAYERS.find((layer) => layer.id === selectedLayerId) ?? MOCK_DESIGN_LAYERS[0],
    [selectedLayerId],
  );

  const composerContextLayerName = useMemo(
    () => composerContextLayerId
      ? MOCK_DESIGN_LAYERS.find((layer) => layer.id === composerContextLayerId)?.name ?? null
      : null,
    [composerContextLayerId],
  );

  const layerRows = useMemo(
    () => MOCK_DESIGN_LAYERS.map((layer) => ({
      ...layer,
      selected: layer.id === selectedLayerId,
      hidden: isHidden(snapshot.hiddenLayerIds, layer.id),
    })),
    [selectedLayerId, snapshot.hiddenLayerIds],
  );

  const radiusOptions = useMemo(
    () => MOCK_DESIGN_RADIUS_OPTIONS.map((option) => ({ ...option, selected: option.value === snapshot.radius })),
    [snapshot.radius],
  );

  const messageViews = useMemo<MessageViewModel[]>(
    () => messages.map((message) => {
      if (message.role === 'user') {
        return { message, icon: '', iconTone: 'done', actions: [] };
      }
      return {
        message,
        icon: message.status === 'working' ? '◌' : message.status === 'error' ? '!' : '✓',
        iconTone: message.status === 'working' ? 'working' : message.status === 'error' ? 'error' : 'done',
        actions: message.status === 'working'
          ? (['stop'] as const)
          : message.status === 'error'
            ? (['retry'] as const)
            : (['select', 'regenerate'] as const),
      };
    }),
    [messages],
  );

  const generationCount = useMemo(
    () => messages.filter((message): message is DesignAssistantMessage => message.role === 'assistant' && message.status === 'done').length,
    [messages],
  );

  const generationLabel = `${generationCount} ${generationCount === 1 ? 'generation' : 'generations'}`;
  const canUndo = history.past.length > 0;
  const canRedo = history.future.length > 0;

  const commitSnapshot = useCallback((change: SnapshotChange) => {
    setHistory((current) => {
      const next = change(current.present);
      if (next === null) return current;
      return {
        ...current,
        present: next,
        past: [...current.past, current.present],
        future: [],
        saved: false,
      };
    });
  }, []);

  const selectLayer = useCallback((layerId: string) => {
    setViewState((current) => current.selectedLayerId === layerId ? current : { ...current, selectedLayerId: layerId });
    setComposerContextLayerId(layerId);
  }, []);

  const toggleLayerVisibility = useCallback((layerId: string) => {
    commitSnapshot((current) => ({
      ...current,
      hiddenLayerIds: current.hiddenLayerIds.includes(layerId)
        ? current.hiddenLayerIds.filter((id) => id !== layerId)
        : [...current.hiddenLayerIds, layerId],
    }));
  }, [commitSnapshot]);

  const setRadius = useCallback((radius: number) => {
    commitSnapshot((current) => current.radius === radius ? null : { ...current, radius });
  }, [commitSnapshot]);

  const setElevation = useCallback((flat: boolean) => {
    commitSnapshot((current) => current.flat === flat ? null : { ...current, flat });
  }, [commitSnapshot]);

  const setMoveTool = useCallback(() => setViewState((current) => current.tool === 'move' ? current : { ...current, tool: 'move' }), []);
  const setAiTool = useCallback(() => setViewState((current) => current.tool === 'ai' ? current : { ...current, tool: 'ai' }), []);

  const setZoom = useCallback((nextZoom: number | ((currentZoom: number) => number)) => {
    setViewState((current) => {
      const next = typeof nextZoom === 'function' ? nextZoom(current.zoom) : nextZoom;
      return current.zoom === next ? current : { ...current, zoom: next };
    });
  }, []);
  const zoomIn = useCallback(() => setZoom((currentZoom) => Math.min(3, Number((currentZoom + 0.1).toFixed(1)))), [setZoom]);
  const zoomOut = useCallback(() => setZoom((currentZoom) => Math.max(0.2, Number((currentZoom - 0.1).toFixed(1)))), [setZoom]);
  const zoomReset = useCallback(() => setZoom(1), [setZoom]);
  const fitCanvas = useCallback(() => setZoom(0.8), [setZoom]);

  const undo = useCallback(() => {
    if (!canUndo) return;
    setHistory((current) => {
      if (current.past.length === 0) return current;
      const previous = current.past[current.past.length - 1];
      return {
        ...current,
        present: previous,
        past: current.past.slice(0, -1),
        future: [current.present, ...current.future],
        saved: false,
      };
    });
  }, [canUndo]);

  const redo = useCallback(() => {
    if (!canRedo) return;
    setHistory((current) => {
      if (current.future.length === 0) return current;
      const next = current.future[0];
      return {
        ...current,
        present: next,
        past: [...current.past, current.present],
        future: current.future.slice(1),
        saved: false,
      };
    });
  }, [canRedo]);

  useEffect(() => {
    const surface = designSurfaceRef.current;
    if (!surface) return;

    const handleKeyDown = (event: globalThis.KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.key.toLowerCase() !== 'z') return;

      const target = event.target;
      if (
        target instanceof HTMLInputElement
        || target instanceof HTMLTextAreaElement
        || (target instanceof HTMLElement && target.isContentEditable)
      ) {
        return;
      }

      if (event.shiftKey) {
        if (!canRedo) return;
        event.preventDefault();
        redo();
        return;
      }

      if (!canUndo) return;
      event.preventDefault();
      undo();
    };

    surface.addEventListener('keydown', handleKeyDown);
    return () => surface.removeEventListener('keydown', handleKeyDown);
  }, [canRedo, canUndo, redo, undo]);

  const save = useCallback(() => setHistory((current) => current.saved ? current : { ...current, saved: true }), []);
  const toggleGrounding = useCallback(() => setGrounded((value) => !value), []);

  const clearGenerationTimer = useCallback(() => {
    if (generationTimerRef.current !== null) {
      window.clearTimeout(generationTimerRef.current);
      generationTimerRef.current = null;
    }
  }, []);

  const startGeneration = useCallback((result: DesignGenerationResult, prompt: string) => {
    if (busy) return;
    clearGenerationTimer();
    const userId = Date.now();
    const assistantId = userId + 1;
    const userMessage: DesignMessage = {
      id: userId,
      role: 'user',
      text: prompt,
      ctx: composerContextLayerName
        ? `${MOCK_DESIGN_DOCUMENT.contextPrefix} ${composerContextLayerName}`
        : undefined,
    };
    const assistantMessage: DesignAssistantMessage = {
      id: assistantId,
      role: 'assistant',
      status: 'working',
      title: MOCK_DESIGN_WORKING_MESSAGE.title,
      desc: MOCK_DESIGN_WORKING_MESSAGE.desc,
      sources: [],
      nodeIds: result.nodeIds,
    };

    setMessages((current) => [...current, userMessage, assistantMessage]);
    setDraft('');
    setBusy(true);
    generationTimerRef.current = window.setTimeout(() => {
      setMessages((current) => current.map((message) => (
        message.id === assistantId && message.role === 'assistant'
          ? { ...message, status: 'done', title: result.title, desc: result.desc, sources: [...result.sources], instruction: prompt }
          : message
      )));
      setBusy(false);
      setHistory((current) => current.saved ? { ...current, saved: false } : current);
      generationTimerRef.current = null;
    }, 900);
  }, [busy, clearGenerationTimer, composerContextLayerName]);

  const send = useCallback(() => {
    const text = draft.trim();
    if (!text || busy) return;
    startGeneration(MOCK_DESIGN_GENERATION_RESULTS.edit, text);
  }, [busy, draft, startGeneration]);

  const visualCheck = useCallback(() => {
    startGeneration(MOCK_DESIGN_GENERATION_RESULTS.visualCheck, MOCK_DESIGN_GENERATION_RESULTS.visualCheck.prompt);
  }, [startGeneration]);

  const handleDraftChange = useCallback((event: ChangeEvent<HTMLTextAreaElement>) => setDraft(event.target.value), []);

  const handleComposerKeyDown = useCallback((event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      send();
    }
  }, [send]);

  const handleMessageAction = useCallback((action: MessageAction, message: DesignMessage) => {
    if (action === 'stop' && message.role === 'assistant') {
      clearGenerationTimer();
      setBusy(false);
      setMessages((current) => current.map((item) => (
        item.id === message.id && item.role === 'assistant'
          ? { ...item, status: 'done', title: 'Stopped', desc: 'Cancelled before the node was written.' }
          : item
      )));
      return;
    }

    if (action === 'select') {
      const nodeId = message.role === 'assistant' ? message.nodeIds[0] : null;
      if (nodeId) selectLayer(nodeId);
      else startGeneration(MOCK_DESIGN_GENERATION_RESULTS.visualCheck, MOCK_DESIGN_GENERATION_RESULTS.visualCheck.prompt);
      return;
    }

    if (action === 'retry') {
      startGeneration(MOCK_DESIGN_GENERATION_RESULTS.retry, MOCK_DESIGN_GENERATION_RESULTS.retry.prompt);
      return;
    }

    startGeneration(MOCK_DESIGN_GENERATION_RESULTS.regenerate, MOCK_DESIGN_GENERATION_RESULTS.regenerate.prompt);
  }, [clearGenerationTimer, selectLayer, startGeneration]);

  const clearComposerContext = useCallback(() => setComposerContextLayerId(null), []);

  return (
    <section ref={designSurfaceRef} className="surface-card design-surface" data-screen-label="Design" aria-labelledby="design-surface-title">
      <h1 className="design-sr-only" id="design-surface-title">Design</h1>
      <DesignToolbar
        grounded={grounded}
        saved={saved}
        busy={busy}
        canUndo={canUndo}
        canRedo={canRedo}
        onGroundingToggle={toggleGrounding}
        onSave={save}
        onUndo={undo}
        onRedo={redo}
      />

      <div className="design-main">
        <div className="design-workspace">
          <DesignCanvas
            hiddenLayerIds={snapshot.hiddenLayerIds}
            selectedLayerId={selectedLayerId}
            tool={tool}
            zoom={zoom}
            onSelectLayer={selectLayer}
          />
          <LayerPanel layers={layerRows} onSelect={selectLayer} onToggleVisibility={toggleLayerVisibility} />
          <div className="design-tool-controls" aria-label="Canvas tools">
            <button type="button" title="Move / select" aria-pressed={tool === 'move'} className={tool === 'move' ? 'design-tool-selected' : ''} onClick={setMoveTool}>Move</button>
            <button type="button" title="Drag a region, then let the AI analyze and fix it" aria-pressed={tool === 'ai'} className={tool === 'ai' ? 'design-tool-selected' : ''} onClick={setAiTool}>Spot Edit</button>
          </div>
          <ZoomControls zoom={zoom} canZoomIn={zoom < 3} canZoomOut={zoom > 0.2} onZoomIn={zoomIn} onZoomOut={zoomOut} onZoomReset={zoomReset} onFit={fitCanvas} />
          <InspectorPanel layer={selectedLayer} radiusOptions={radiusOptions} flat={snapshot.flat} onRadiusChange={setRadius} onElevationChange={setElevation} />
        </div>

        <DesignAssistant
          generationLabel={generationLabel}
          contextLayerName={composerContextLayerName}
          provider={MOCK_DESIGN_DOCUMENT.provider}
          draft={draft}
          draftPlaceholder={composerContextLayerName ? MOCK_DESIGN_DOCUMENT.draftPlaceholder : MOCK_DESIGN_DOCUMENT.noContextPlaceholder}
          sendLabel={busy ? 'Working…' : 'Generate'}
          busy={busy}
          messages={messageViews}
          assistantRef={assistantRef}
          onDraftChange={handleDraftChange}
          onComposerKeyDown={handleComposerKeyDown}
          onSend={send}
          onVisualCheck={visualCheck}
          onClearContext={clearComposerContext}
          onMessageAction={handleMessageAction}
        />
      </div>
    </section>
  );
}
