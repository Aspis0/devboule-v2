import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  ChangeEvent,
  KeyboardEvent,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  RefObject,
} from "react";
import type {
  DesignAssistantMessage,
  DesignCanvasContent,
  DesignDocument,
  DesignHost,
  DesignLayer,
  DesignMessage,
  DesignRadiusOption,
  DesignTool,
} from "./designHost";
import { hitTest } from "../../lib/canvas/hitTest";
import { nodesBounds, type Pan } from "../../lib/canvas/viewportMath";
import type { NodeRect } from "../../types/geometry";
import {
  clampViewportZoom,
  createViewport,
  createViewportCommitScheduler,
  DESIGN_MAX_ZOOM,
  DESIGN_MIN_ZOOM,
  fitViewport,
  panViewport,
  pointerToWorld,
  viewportTransform,
  zoomViewport,
  type DesignViewport,
} from "./designViewport";
import "./design.css";

export type { DesignDocument, DesignHost } from "./designHost";

type MessageAction = "stop" | "retry" | "select" | "regenerate";

interface DesignSnapshot {
  hiddenLayerIds: readonly string[];
  layers: readonly DesignLayer[];
  radius: number;
  flat: boolean;
}

interface DesignViewState {
  // Viewport state is deliberately outside DesignHistory, so undo never moves the camera.
  pan: Pan;
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
  iconTone: "working" | "error" | "done";
  actions: readonly MessageAction[];
}

interface DesignToolbarProps {
  documentName: string;
  documentPath: string;
  grounded: boolean;
  canSave: boolean;
  saved: boolean;
  saving: boolean;
  saveError: string | null;
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
  content: DesignCanvasContent;
  layers: readonly DesignLayer[];
  hiddenLayerIds: readonly string[];
  pan: Pan;
  selectedLayerId: string;
  tool: DesignTool;
  zoom: number;
  onSelectLayer: (layerId: string) => void;
  onViewportChange: (viewport: DesignViewport) => void;
}

interface CanvasNodeProps {
  layer: DesignLayer;
  hidden: boolean;
  selected: boolean;
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
  tokenFooter: string;
  radiusOptions: readonly RadiusViewModel[];
  flat: boolean;
  onRadiusChange: (radius: number) => void;
  onElevationChange: (flat: boolean) => void;
  onDuplicate: () => void;
  onDelete: () => void;
  canDelete: boolean;
}

interface AssistantProps {
  canGenerate: boolean;
  contextPrefix: string;
  generationLabel: string;
  contextLayerName: string | null;
  provider: string;
  draft: string;
  draftPlaceholder: string;
  sendLabel: string;
  busy: boolean;
  messages: readonly MessageViewModel[];
  assistantRef: RefObject<HTMLDivElement | null>;
  onDraftChange: (event: ChangeEvent<HTMLTextAreaElement>) => void;
  onComposerKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  onSend: () => void;
  onVisualCheck: () => void;
  onClearContext: () => void;
  onMessageAction: (action: MessageAction, message: DesignMessage) => void;
}

type SnapshotChange = (current: DesignSnapshot) => DesignSnapshot | null;

function cloneMessages(document: DesignDocument): DesignMessage[] {
  return cloneMessageList(document.messages);
}

function cloneMessageList(messages: readonly DesignMessage[]): DesignMessage[] {
  return messages.map((message) => {
    if (message.role === "user") return { ...message };
    return {
      ...message,
      sources: [...message.sources],
      nodeIds: [...message.nodeIds],
    };
  });
}

function cloneLayers(document: DesignDocument): DesignLayer[] {
  return cloneLayerList(document.layers);
}

function cloneLayerList(layers: readonly DesignLayer[]): DesignLayer[] {
  return layers.map((layer) => ({
    ...layer,
    transform: { ...layer.transform },
  }));
}

function isHidden(hiddenLayerIds: readonly string[], layerId: string): boolean {
  return hiddenLayerIds.includes(layerId);
}

function promptForMessage(
  messages: readonly DesignMessage[],
  message: DesignMessage,
): string | null {
  if (message.role !== "assistant") return null;
  const messageIndex = messages.findIndex((candidate) => candidate.id === message.id);
  const previousMessage = messageIndex > 0 ? messages[messageIndex - 1] : undefined;
  return previousMessage?.role === "user" ? previousMessage.text : (message.instruction ?? null);
}

const DesignToolbar = memo(function DesignToolbar({
  documentName,
  documentPath,
  grounded,
  canSave,
  saved,
  saving,
  saveError,
  canUndo,
  canRedo,
  onGroundingToggle,
  onSave,
  onUndo,
  onRedo,
}: DesignToolbarProps) {
  const saveText = saving ? "Saving…" : saved ? "Saved" : "Unsaved changes";

  return (
    <header className="design-toolbar">
      <button className="design-browser-selector" type="button" aria-label="Choose design document">
        <span className="design-toolbar-dot" aria-hidden="true" />
        <span className="design-browser-name">{documentName}</span>
        <span className="design-chevron" aria-hidden="true">
          ▾
        </span>
      </button>
      <span className="design-path" title={documentPath}>
        {documentPath}
      </span>
      {canSave ? (
        <>
          <span className="design-save-status" aria-live="polite">
            <span
              className={`design-save-dot${saved && !saving ? " design-save-dot-saved" : ""}`}
              aria-hidden="true"
            />
            {saveText}
          </span>
          {saveError ? (
            <span className="design-save-error" role="alert">
              {saveError}
            </span>
          ) : null}
        </>
      ) : null}
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
        <span
          className={`design-grounding-dot${grounded ? " design-grounding-dot-on" : ""}`}
          aria-hidden="true"
        />
        {grounded ? "Grounded · devboule" : "Not grounded"}
        <span className="design-chevron" aria-hidden="true">
          ▾
        </span>
      </button>
      <button className="design-toolbar-button" type="button">
        Export
      </button>
      <button className="design-toolbar-button" type="button">
        Preview
      </button>
      {canSave ? (
        <span className="design-save-actions">
          <button className="design-save-primary" type="button" onClick={onSave} disabled={saving}>
            Save to repo
          </button>
          <button
            className="design-save-menu"
            type="button"
            title="More save options"
            aria-label="More save options"
          >
            ▾
          </button>
        </span>
      ) : null}
    </header>
  );
});

const LayerPanel = memo(function LayerPanel({
  layers,
  onSelect,
  onToggleVisibility,
}: LayerPanelProps) {
  return (
    <section className="design-layers-panel" aria-labelledby="design-layers-title">
      <div className="design-overlay-heading">
        <span id="design-layers-title">Layers</span>
        <span className="design-layer-count">{layers.length}</span>
        <span className="design-chevron" aria-hidden="true">
          ▾
        </span>
      </div>
      <div className="design-layer-list">
        {layers.map((layer) => (
          <div
            className={`design-layer-row${layer.selected ? " design-layer-row-selected" : ""}`}
            key={layer.id}
          >
            <button
              className="design-layer-select"
              type="button"
              aria-pressed={layer.selected}
              aria-label={`Select ${layer.name}`}
              onClick={() => onSelect(layer.id)}
            >
              <span className="design-layer-kind">{layer.kind}</span>
              <span
                className={`design-layer-name${layer.hidden ? " design-layer-name-hidden" : ""}`}
              >
                {layer.name}
              </span>
            </button>
            <button
              className="design-layer-visibility"
              type="button"
              aria-pressed={!layer.hidden}
              aria-label={`${layer.hidden ? "Show" : "Hide"} ${layer.name}`}
              title="Hide / show"
              onClick={() => onToggleVisibility(layer.id)}
            >
              {layer.hidden ? "◌" : "◉"}
            </button>
          </div>
        ))}
      </div>
    </section>
  );
});

const CanvasNode = memo(function CanvasNode({ layer, hidden, selected }: CanvasNodeProps) {
  return (
    <button
      className={`design-canvas-node${selected ? " design-canvas-node-selected" : ""}${hidden ? " design-canvas-node-hidden" : ""}`}
      type="button"
      style={{
        left: layer.transform.x,
        top: layer.transform.y,
        width: layer.transform.width,
        height: layer.transform.height,
      }}
      data-canvas-layer-id={layer.id}
      aria-label={`Select ${layer.name}`}
      aria-pressed={selected}
      disabled={hidden}
    >
      <div className="design-canvas-node-body">
        <div className="design-node-heading">
          <span
            className={`design-node-mark ${layer.kind === "SVG" ? "design-node-mark-purple" : "design-node-mark-terracotta"}`}
            aria-hidden="true"
          />
          <span className="design-node-title">{layer.name}</span>
          <span className="design-node-badge">{layer.kind}</span>
        </div>
        <div className="design-node-actions">
          <span className="design-node-primary-action">
            {layer.transform.width} × {layer.transform.height}
          </span>
          <span className="design-node-secondary-action">World layer</span>
        </div>
      </div>
      {selected ? (
        <>
          <span className="design-selection-handle design-selection-handle-tl" aria-hidden="true" />
          <span className="design-selection-handle design-selection-handle-tr" aria-hidden="true" />
          <span className="design-selection-handle design-selection-handle-bl" aria-hidden="true" />
          <span className="design-selection-handle design-selection-handle-br" aria-hidden="true" />
        </>
      ) : null}
    </button>
  );
});

const ZoomControls = memo(function ZoomControls({
  zoom,
  canZoomIn,
  canZoomOut,
  onZoomIn,
  onZoomOut,
  onZoomReset,
  onFit,
}: ZoomControlsProps) {
  const zoomLabel = `${Math.round(zoom * 100)}%`;

  return (
    <div className="design-zoom-controls" aria-label="Canvas zoom">
      <button
        type="button"
        title="Zoom out"
        aria-label="Zoom out"
        onClick={onZoomOut}
        disabled={!canZoomOut}
      >
        −
      </button>
      <button
        className="design-zoom-value"
        type="button"
        title="Reset to 100%"
        aria-label="Reset zoom to 100%"
        onClick={onZoomReset}
      >
        {zoomLabel}
      </button>
      <button
        type="button"
        title="Zoom in"
        aria-label="Zoom in"
        onClick={onZoomIn}
        disabled={!canZoomIn}
      >
        +
      </button>
      <button className="design-fit-button" type="button" title="Fit canvas" onClick={onFit}>
        Fit
      </button>
    </div>
  );
});

const DesignCanvas = memo(function DesignCanvas({
  content,
  layers,
  hiddenLayerIds,
  pan,
  selectedLayerId,
  tool,
  zoom,
  onSelectLayer,
  onViewportChange,
}: CanvasProps) {
  const aiRegion = content.aiRegion;
  const canvasRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<DesignViewport>(createViewport(zoom, pan));
  const pointerDragRef = useRef<{
    button: number;
    moved: boolean;
    pointerId: number;
    lastX: number;
    lastY: number;
  } | null>(null);
  const suppressClickRef = useRef(false);

  const viewportCommitScheduler = useMemo(
    () =>
      createViewportCommitScheduler(
        onViewportChange,
        (callback) => window.requestAnimationFrame(callback),
        (frameId) => window.cancelAnimationFrame(frameId),
      ),
    [onViewportChange],
  );

  // Pointer moves update one stage transform imperatively; React records only settled viewport changes.
  const applyViewport = useCallback((next: DesignViewport) => {
    viewportRef.current = next;
    if (stageRef.current) stageRef.current.style.transform = viewportTransform(next);
  }, []);

  useEffect(() => {
    // While a drag is active, React may receive a zoom-button update before the
    // uncommitted pan does. Preserve the imperative pan so that update composes.
    const appliedPan = pointerDragRef.current ? viewportRef.current.pan : pan;
    applyViewport(createViewport(zoom, appliedPan));
  }, [applyViewport, pan, zoom]);

  const layerRects = useMemo<NodeRect[]>(
    () =>
      layers
        .map((layer, index) => ({
          id: layer.id,
          x: layer.transform.x,
          y: layer.transform.y,
          w: layer.transform.width,
          h: layer.transform.height,
          z: index,
        }))
        .filter((layer) => !hiddenLayerIds.includes(layer.id)),
    [hiddenLayerIds, layers],
  );

  const handleCanvasClick = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      if (suppressClickRef.current) {
        suppressClickRef.current = false;
        return;
      }
      const canvas = canvasRef.current;
      if (!canvas) return;
      const bounds = canvas.getBoundingClientRect();
      const point = pointerToWorld(
        event.clientX,
        event.clientY,
        { left: bounds.left, top: bounds.top },
        viewportRef.current,
      );
      let target = hitTest(point, layerRects);
      if (!target && event.target instanceof Element) {
        const clickedNode = event.target.closest<HTMLElement>("[data-canvas-layer-id]");
        const clickedRect = layerRects.find(
          (layer) => layer.id === clickedNode?.dataset.canvasLayerId,
        );
        if (clickedRect) {
          onSelectLayer(clickedRect.id);
          return;
        }
      }
      onSelectLayer(target?.id ?? "");
    },
    [layerRects, onSelectLayer],
  );

  const handleWheel = useCallback(
    (event: WheelEvent) => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      event.preventDefault();
      const bounds = canvas.getBoundingClientRect();
      const next = zoomViewport(
        viewportRef.current,
        { deltaY: event.deltaY, deltaMode: event.deltaMode },
        event.clientX - bounds.left,
        event.clientY - bounds.top,
        bounds.height,
      );
      applyViewport(next);
      viewportCommitScheduler.schedule(next);
    },
    [applyViewport, viewportCommitScheduler],
  );

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    canvas.addEventListener("wheel", handleWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", handleWheel);
  }, [handleWheel]);

  useEffect(() => () => viewportCommitScheduler.cancel(), [viewportCommitScheduler]);

  const releasePointer = useCallback((element: HTMLDivElement, pointerId: number) => {
    try {
      if (element.hasPointerCapture(pointerId)) element.releasePointerCapture(pointerId);
    } catch {
      // Pointer capture may already be gone when the browser cancels the gesture.
    }
  }, []);

  const finishPointerDrag = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>, followsClick: boolean) => {
      const active = pointerDragRef.current;
      if (!active || active.pointerId !== event.pointerId) return;
      pointerDragRef.current = null;
      releasePointer(event.currentTarget, event.pointerId);
      if (active.moved && active.button === 0 && followsClick) suppressClickRef.current = true;
      if (active.moved) viewportCommitScheduler.flush(viewportRef.current);
    },
    [releasePointer, viewportCommitScheduler],
  );

  const handlePointerUp = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => finishPointerDrag(event, true),
    [finishPointerDrag],
  );
  const handlePointerCancel = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => finishPointerDrag(event, false),
    [finishPointerDrag],
  );
  const handleLostPointerCapture = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => finishPointerDrag(event, false),
    [finishPointerDrag],
  );

  const handlePointerDown = useCallback((event: ReactPointerEvent<HTMLDivElement>) => {
    const startedOnEmptyCanvas = event.button === 0 && event.target === event.currentTarget;
    if (!startedOnEmptyCanvas && event.button !== 1) return;
    event.preventDefault();

    pointerDragRef.current = {
      button: event.button,
      moved: false,
      pointerId: event.pointerId,
      lastX: event.clientX,
      lastY: event.clientY,
    };
    try {
      event.currentTarget.setPointerCapture(event.pointerId);
    } catch {
      pointerDragRef.current = null;
    }
  }, []);

  const handlePointerMove = useCallback(
    (event: ReactPointerEvent<HTMLDivElement>) => {
      const active = pointerDragRef.current;
      if (!active || active.pointerId !== event.pointerId) return;
      try {
        const delta = { x: event.clientX - active.lastX, y: event.clientY - active.lastY };
        active.lastX = event.clientX;
        active.lastY = event.clientY;
        active.moved = active.moved || delta.x !== 0 || delta.y !== 0;
        applyViewport(panViewport(viewportRef.current, delta));
      } catch {
        finishPointerDrag(event, false);
      }
    },
    [applyViewport, finishPointerDrag],
  );

  const cleanupPointerDrag = useCallback(() => {
    const active = pointerDragRef.current;
    const stage = stageRef.current;
    pointerDragRef.current = null;
    if (active && stage) releasePointer(stage, active.pointerId);
  }, [releasePointer]);

  useEffect(() => cleanupPointerDrag, [cleanupPointerDrag]);

  return (
    <div
      ref={canvasRef}
      className="design-canvas"
      aria-label="Design canvas"
      onClick={handleCanvasClick}
    >
      <div className="design-canvas-grid" aria-hidden="true" />
      <div
        ref={stageRef}
        className="design-canvas-stage"
        style={{ transform: viewportTransform({ pan, zoom }) }}
        onPointerDown={handlePointerDown}
        onPointerMove={handlePointerMove}
        onPointerUp={handlePointerUp}
        onPointerCancel={handlePointerCancel}
        onLostPointerCapture={handleLostPointerCapture}
      >
        {layers.map((layer) => (
          <CanvasNode
            key={layer.id}
            layer={layer}
            hidden={isHidden(hiddenLayerIds, layer.id)}
            selected={selectedLayerId === layer.id}
          />
        ))}
        {tool === "ai" ? (
          <div
            className="design-ai-region"
            style={{
              left: aiRegion.x,
              top: aiRegion.y,
              width: aiRegion.width,
              height: aiRegion.height,
            }}
          >
            <button type="button">{aiRegion.actionLabel}</button>
          </div>
        ) : null}
      </div>
    </div>
  );
});

const InspectorPanel = memo(function InspectorPanel({
  layer,
  tokenFooter,
  radiusOptions,
  flat,
  onRadiusChange,
  onElevationChange,
  onDuplicate,
  onDelete,
  canDelete,
}: InspectorProps) {
  const transformFields = [
    ["X", layer.transform.x],
    ["Y", layer.transform.y],
    ["W", layer.transform.width],
    ["H", layer.transform.height],
  ] as const;

  return (
    <section className="design-inspector-panel" aria-labelledby="design-inspector-title">
      <div className="design-inspector-heading">
        <span id="design-inspector-title" className="design-inspector-name">
          {layer.name}
        </span>
        <span className="design-inspector-kind">{layer.kind}</span>
        <span className="design-inspector-close" aria-hidden="true">
          ✕
        </span>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label">Transform</div>
        <div className="design-transform-grid">
          {transformFields.map(([label, value]) => (
            <span className="design-transform-field" key={label}>
              <span className="design-field-label">{label}</span>
              <span className="design-mono-value">{value}</span>
              {label === "H" && layer.transform.hug ? (
                <span className="design-hug-label">HUG</span>
              ) : null}
            </span>
          ))}
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label-row">
          <span className="design-inspector-label">Corners</span>
          <span className="design-token-value">
            radius.{radiusOptions.find((option) => option.selected)?.token ?? "custom"}
          </span>
        </div>
        <div className="design-radius-picker">
          {radiusOptions.map((option) => (
            <button
              className={`design-radius-option${option.selected ? " design-radius-option-selected" : ""}`}
              type="button"
              key={option.token}
              title={`radius.${option.token} · ${option.value}px`}
              aria-label={`Set radius.${option.token}, ${option.value} pixels`}
              aria-pressed={option.selected}
              onClick={() => onRadiusChange(option.value)}
            >
              <span
                className="design-radius-glyph"
                style={{ borderTopLeftRadius: `${Math.min(option.value, 12)}px` }}
                aria-hidden="true"
              />
            </button>
          ))}
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label-row">
          <span className="design-inspector-label">Elevation</span>
          <span className="design-token-value">{flat ? "shadow.none" : "shadow.soft"}</span>
        </div>
        <div className="design-segmented-control">
          <button
            type="button"
            aria-pressed={!flat}
            className={!flat ? "design-segment-selected" : ""}
            onClick={() => onElevationChange(false)}
          >
            Soft
          </button>
          <button
            type="button"
            aria-pressed={flat}
            className={flat ? "design-segment-selected" : ""}
            onClick={() => onElevationChange(true)}
          >
            Flat
          </button>
        </div>
      </div>

      <div className="design-inspector-section">
        <div className="design-inspector-label">Arrange</div>
        <div className="design-arrange-actions">
          <button type="button" title="Send to back" aria-label="Send layer to back">
            ⤓
          </button>
          <button type="button" title="Move backward" aria-label="Move layer backward">
            ↓
          </button>
          <button type="button" title="Move forward" aria-label="Move layer forward">
            ↑
          </button>
          <button type="button" title="Bring to front" aria-label="Bring layer to front">
            ⤒
          </button>
        </div>
      </div>

      <div className="design-inspector-actions">
        <button type="button" onClick={onDuplicate}>
          Duplicate
        </button>
        <button
          type="button"
          className="design-delete-action"
          aria-label="Delete layer"
          onClick={onDelete}
          disabled={!canDelete}
        >
          Delete
        </button>
      </div>
      <div className="design-inspector-footer">{tokenFooter}</div>
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

  if (message.role === "user") {
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
        <span
          className={`design-message-icon design-message-icon-${view.iconTone}`}
          aria-hidden="true"
        >
          {view.icon}
        </span>
        <span className="design-message-title">{message.title}</span>
      </div>
      <div className="design-message-description">{message.desc}</div>
      {message.sources.length > 0 ? (
        <div className="design-message-sources">
          {message.sources.map((source) => (
            <span key={source}>{source}</span>
          ))}
        </div>
      ) : null}
      <div className="design-message-actions">
        {view.actions.map((action) => (
          <button type="button" key={action} onClick={() => onAction(action, message)}>
            {action === "stop"
              ? "Stop"
              : action === "retry"
                ? "Retry"
                : action === "select"
                  ? "Select on canvas"
                  : "Regenerate"}
          </button>
        ))}
      </div>
    </div>
  );
});

const DesignAssistant = memo(function DesignAssistant({
  canGenerate,
  contextPrefix,
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
        <span id="design-assistant-title" className="design-assistant-title">
          Assistant
        </span>
        <span className="design-generation-label">{generationLabel}</span>
        {canGenerate ? (
          <button
            className="design-visual-check"
            type="button"
            title="Visual check"
            aria-label="Run visual check"
            onClick={onVisualCheck}
          >
            ◉
          </button>
        ) : null}
      </div>

      <div className="design-assistant-scroll design-scroll" ref={assistantRef}>
        {messages.map((view) => (
          <DesignMessageCard key={view.message.id} view={view} onAction={onMessageAction} />
        ))}
      </div>

      {canGenerate ? (
        <div className="design-composer-wrap">
          <div className="design-composer">
            {contextLayerName ? (
              <div className="design-composer-context">
                <span>
                  {contextPrefix} {contextLayerName}
                </span>
                <button
                  type="button"
                  title="Clear context"
                  aria-label="Clear editing context"
                  onClick={onClearContext}
                >
                  ✕
                </button>
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
              <button
                className="design-provider-button"
                type="button"
                aria-label={`Model: ${provider}`}
              >
                <span className="design-provider-dot" aria-hidden="true" />
                {provider} ▾
              </button>
              <button
                className="design-generate-button"
                type="button"
                onClick={onSend}
                disabled={busy || !draft.trim()}
              >
                {sendLabel}
              </button>
            </div>
          </div>
          <div className="design-composer-hint">
            <b>Enter</b> to send · <b>Shift+Enter</b> for a new line
          </div>
        </div>
      ) : null}
    </aside>
  );
});

export interface DesignSurfaceProps {
  host: DesignHost;
  disclosure?: string;
}

export function DesignSurface({ host, disclosure }: DesignSurfaceProps) {
  const [document, setDocument] = useState<DesignDocument | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    void host
      .loadDocument()
      .then((loadedDocument) => {
        if (!active) return;
        setDocument(loadedDocument);
      })
      .catch((error: unknown) => {
        if (!active) return;
        setLoadError(
          error instanceof Error ? error.message : "The design document could not load.",
        );
      });

    return () => {
      active = false;
    };
  }, [host]);

  if (loadError !== null) {
    return (
      <section className="surface-card design-surface" data-screen-label="Design">
        {disclosure ? <div className="design-demo-disclosure">{disclosure}</div> : null}
        <div role="alert">Unable to load the design document: {loadError}</div>
      </section>
    );
  }

  if (document === null) {
    return (
      <section className="surface-card design-surface" data-screen-label="Design">
        {disclosure ? <div className="design-demo-disclosure">{disclosure}</div> : null}
        <div role="status">Loading…</div>
      </section>
    );
  }

  return <DesignSurfaceContent host={host} document={document} disclosure={disclosure} />;
}

interface DesignSurfaceContentProps {
  host: DesignHost;
  document: DesignDocument;
  disclosure?: string;
}

function DesignSurfaceContent({ host, document, disclosure }: DesignSurfaceContentProps) {
  const initialSnapshot: DesignSnapshot = {
    hiddenLayerIds: document.initialState.hiddenLayerIds,
    layers: cloneLayers(document),
    radius: document.initialState.radius,
    flat: document.initialState.flat,
  };
  const initialViewState: DesignViewState = {
    pan: { x: 0, y: 0 },
    selectedLayerId: document.initialState.selectedLayerId,
    tool: document.initialState.tool,
    zoom: clampViewportZoom(document.initialState.zoom),
  };
  const [history, setHistory] = useState<DesignHistory>(() => ({
    present: initialSnapshot,
    past: [],
    future: [],
    saved: document.initialState.saved,
  }));
  const [viewState, setViewState] = useState<DesignViewState>(initialViewState);
  const [composerContextLayerId, setComposerContextLayerId] = useState<string | null>(
    initialViewState.selectedLayerId,
  );
  const [grounded, setGrounded] = useState(document.initialState.grounded);
  const [draft, setDraft] = useState(document.initialState.draft);
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [messages, setMessages] = useState<DesignMessage[]>(() => cloneMessages(document));

  const activeGenerationRef = useRef<{
    controller: AbortController;
    delivered: boolean;
  } | null>(null);
  const documentRevisionRef = useRef(0);
  const layerCopyCounterRef = useRef(0);
  const assistantRef = useRef<HTMLDivElement>(null);
  const designSurfaceRef = useRef<HTMLElement>(null);

  const snapshot = history.present;
  const layers = snapshot.layers;
  const saved = history.saved;
  const { pan, selectedLayerId, tool, zoom } = viewState;

  useEffect(() => {
    return () => {
      activeGenerationRef.current?.controller.abort();
    };
  }, []);

  useEffect(() => {
    if (assistantRef.current) assistantRef.current.scrollTop = assistantRef.current.scrollHeight;
  }, [messages, busy]);

  const selectedLayer = useMemo(
    () => layers.find((layer) => layer.id === selectedLayerId) ?? layers[0],
    [layers, selectedLayerId],
  );

  const composerContextLayerName = useMemo(
    () =>
      composerContextLayerId
        ? (layers.find((layer) => layer.id === composerContextLayerId)?.name ?? null)
        : null,
    [composerContextLayerId, layers],
  );

  const layerRows = useMemo(
    () =>
      layers.map((layer) => ({
        ...layer,
        selected: layer.id === selectedLayerId,
        hidden: isHidden(snapshot.hiddenLayerIds, layer.id),
      })),
    [layers, selectedLayerId, snapshot.hiddenLayerIds],
  );

  const fitRects = useMemo<NodeRect[]>(
    () =>
      layers
        .map((layer, index) => ({
          id: layer.id,
          x: layer.transform.x,
          y: layer.transform.y,
          w: layer.transform.width,
          h: layer.transform.height,
          z: index,
        }))
        .filter((layer) => !snapshot.hiddenLayerIds.includes(layer.id)),
    [layers, snapshot.hiddenLayerIds],
  );

  const radiusOptions = useMemo(
    () =>
      document.radiusOptions.map((option) => ({
        ...option,
        selected: option.value === snapshot.radius,
      })),
    [document.radiusOptions, snapshot.radius],
  );

  const saveDocument = host.saveDocument;
  const generate = host.generate;
  const canSave = saveDocument !== undefined;
  const canGenerate = generate !== undefined;

  const messageViews = useMemo<MessageViewModel[]>(
    () =>
      messages.map((message) => {
        if (message.role === "user") {
          return { message, icon: "", iconTone: "done", actions: [] };
        }
        return {
          message,
          icon: message.status === "working" ? "◌" : message.status === "error" ? "!" : "✓",
          iconTone:
            message.status === "working"
              ? "working"
              : message.status === "error"
                ? "error"
                : "done",
          actions:
            message.status === "working"
              ? canGenerate
                ? (["stop"] as const)
                : []
              : message.status === "error"
                ? canGenerate
                  ? (["retry"] as const)
                  : []
                : canGenerate
                  ? (["select", "regenerate"] as const)
                  : (["select"] as const),
        };
      }),
    [canGenerate, messages],
  );

  const generationCount = useMemo(
    () =>
      messages.filter(
        (message): message is DesignAssistantMessage =>
          message.role === "assistant" && message.status === "done",
      ).length,
    [messages],
  );

  const generationLabel = `${generationCount} ${generationCount === 1 ? "generation" : "generations"}`;
  const canUndo = history.past.length > 0;
  const canRedo = history.future.length > 0;

  const commitSnapshot = useCallback((change: SnapshotChange) => {
    setHistory((current) => {
      const next = change(current.present);
      if (next === null) return current;
      documentRevisionRef.current += 1;
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
    setViewState((current) =>
      current.selectedLayerId === layerId ? current : { ...current, selectedLayerId: layerId },
    );
    setComposerContextLayerId(layerId);
  }, []);

  const duplicateLayer = useCallback(() => {
    const source = layers.find((layer) => layer.id === selectedLayerId);
    if (!source) return;

    layerCopyCounterRef.current += 1;
    const copy: DesignLayer = {
      ...source,
      id: `${source.id}-copy-${layerCopyCounterRef.current}`,
      name: `${source.name} copy`,
      transform: {
        ...source.transform,
        x: source.transform.x + 16,
        y: source.transform.y + 16,
      },
    };
    commitSnapshot((current) => ({ ...current, layers: [...current.layers, copy] }));
    selectLayer(copy.id);
  }, [commitSnapshot, layers, selectedLayerId, selectLayer]);

  const deleteLayer = useCallback(() => {
    if (layers.length <= 1) return;

    const selectedIndex = layers.findIndex((layer) => layer.id === selectedLayerId);
    const nextLayer = layers[selectedIndex + 1] ?? layers[selectedIndex - 1];
    if (!nextLayer) return;

    selectLayer(nextLayer.id);
    commitSnapshot((current) => ({
      ...current,
      layers: current.layers.filter((layer) => layer.id !== selectedLayerId),
      hiddenLayerIds: current.hiddenLayerIds.filter((id) => id !== selectedLayerId),
    }));
  }, [commitSnapshot, layers, selectedLayerId, selectLayer]);

  const toggleLayerVisibility = useCallback(
    (layerId: string) => {
      commitSnapshot((current) => ({
        ...current,
        hiddenLayerIds: current.hiddenLayerIds.includes(layerId)
          ? current.hiddenLayerIds.filter((id) => id !== layerId)
          : [...current.hiddenLayerIds, layerId],
      }));
    },
    [commitSnapshot],
  );

  const setRadius = useCallback(
    (radius: number) => {
      commitSnapshot((current) => (current.radius === radius ? null : { ...current, radius }));
    },
    [commitSnapshot],
  );

  const setElevation = useCallback(
    (flat: boolean) => {
      commitSnapshot((current) => (current.flat === flat ? null : { ...current, flat }));
    },
    [commitSnapshot],
  );

  const setMoveTool = useCallback(
    () =>
      setViewState((current) => (current.tool === "move" ? current : { ...current, tool: "move" })),
    [],
  );
  const setAiTool = useCallback(
    () => setViewState((current) => (current.tool === "ai" ? current : { ...current, tool: "ai" })),
    [],
  );

  const setViewport = useCallback((nextViewport: DesignViewport) => {
    setViewState((current) => {
      if (
        current.zoom === nextViewport.zoom &&
        current.pan.x === nextViewport.pan.x &&
        current.pan.y === nextViewport.pan.y
      ) {
        return current;
      }
      return { ...current, ...nextViewport };
    });
  }, []);
  const setZoom = useCallback((nextZoom: number | ((currentZoom: number) => number)) => {
    setViewState((current) => {
      const requested = typeof nextZoom === "function" ? nextZoom(current.zoom) : nextZoom;
      const next = clampViewportZoom(requested);
      return current.zoom === next ? current : { ...current, zoom: next };
    });
  }, []);
  const zoomIn = useCallback(
    () => setZoom((currentZoom) => Number((currentZoom + 0.1).toFixed(1))),
    [setZoom],
  );
  const zoomOut = useCallback(
    () => setZoom((currentZoom) => Number((currentZoom - 0.1).toFixed(1))),
    [setZoom],
  );
  const zoomReset = useCallback(() => setZoom(1), [setZoom]);
  const fitCanvas = useCallback(() => {
    const canvas = designSurfaceRef.current?.querySelector<HTMLElement>(".design-canvas");
    if (!canvas) return;
    const bounds = canvas.getBoundingClientRect();
    setViewport(fitViewport(nodesBounds(fitRects), bounds.width, bounds.height));
  }, [fitRects, setViewport]);

  const undo = useCallback(() => {
    if (!canUndo) return;
    setHistory((current) => {
      if (current.past.length === 0) return current;
      const previous = current.past[current.past.length - 1];
      documentRevisionRef.current += 1;
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
      documentRevisionRef.current += 1;
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
      if (!event.ctrlKey || event.altKey || event.key.toLowerCase() !== "z") return;

      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable)
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

    surface.addEventListener("keydown", handleKeyDown);
    return () => surface.removeEventListener("keydown", handleKeyDown);
  }, [canRedo, canUndo, redo, undo]);

  const save = useCallback(async () => {
    if (saveDocument === undefined || saving) return;

    const revisionAtSave = documentRevisionRef.current;
    const documentToSave: DesignDocument = {
      ...document,
      initialState: {
        ...document.initialState,
        hiddenLayerIds: [...history.present.hiddenLayerIds],
        radius: history.present.radius,
        flat: history.present.flat,
      },
      layers: cloneLayerList(layers),
      messages: cloneMessageList(messages),
    };

    setSaveError(null);
    setSaving(true);
    try {
      await saveDocument(documentToSave);
      if (documentRevisionRef.current === revisionAtSave) {
        setHistory((current) => (current.saved ? current : { ...current, saved: true }));
      }
    } catch (error: unknown) {
      setHistory((current) => (current.saved ? { ...current, saved: false } : current));
      setSaveError(error instanceof Error ? error.message : "The design document could not save.");
    } finally {
      setSaving(false);
    }
  }, [document, history.present, layers, messages, saveDocument, saving]);
  const toggleGrounding = useCallback(() => setGrounded((value) => !value), []);

  const startGeneration = useCallback(
    (prompt: string) => {
      if (busy || generate === undefined) return;
      activeGenerationRef.current?.controller.abort();
      const controller = new AbortController();
      const activeGeneration = { controller, delivered: false };
      const userId = Date.now();
      const assistantId = userId + 1;
      const userMessage: DesignMessage = {
        id: userId,
        role: "user",
        text: prompt,
        ctx: composerContextLayerName
          ? `${document.contextPrefix} ${composerContextLayerName}`
          : undefined,
      };
      const assistantMessage: DesignAssistantMessage = {
        id: assistantId,
        role: "assistant",
        status: "working",
        title: document.workingMessage.title,
        desc: document.workingMessage.desc,
        sources: [],
        nodeIds: [],
        instruction: prompt,
      };

      activeGenerationRef.current = activeGeneration;
      documentRevisionRef.current += 1;
      setMessages((current) => [...current, userMessage, assistantMessage]);
      setDraft("");
      setBusy(true);
      void generate(prompt, controller.signal)
        .then((result) => {
          if (controller.signal.aborted) return;
          activeGeneration.delivered = true;
          documentRevisionRef.current += 1;
          setMessages((current) =>
            current.map((message) =>
              message.id === assistantId && message.role === "assistant"
                ? {
                    ...message,
                    status: "done",
                    title: result.title,
                    desc: result.desc,
                    sources: [...result.sources],
                    nodeIds: [...result.nodeIds],
                    instruction: prompt,
                  }
                : message,
            ),
          );
          setBusy(false);
          setHistory((current) => (current.saved ? { ...current, saved: false } : current));
          activeGenerationRef.current = null;
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted) return;
          documentRevisionRef.current += 1;
          setMessages((current) =>
            current.map((message) =>
              message.id === assistantId && message.role === "assistant"
                ? {
                    ...message,
                    status: "error",
                    title: "Generation failed",
                    desc: error instanceof Error ? error.message : "The design generation failed.",
                  }
                : message,
            ),
          );
          setBusy(false);
          setHistory((current) => (current.saved ? { ...current, saved: false } : current));
          activeGenerationRef.current = null;
        });
    },
    [busy, composerContextLayerName, document.contextPrefix, document.workingMessage, generate],
  );

  const send = useCallback(() => {
    const text = draft.trim();
    if (!text || busy) return;
    startGeneration(text);
  }, [busy, draft, startGeneration]);

  const visualCheck = useCallback(() => {
    startGeneration("Run a visual check on the canvas.");
  }, [startGeneration]);

  const handleDraftChange = useCallback(
    (event: ChangeEvent<HTMLTextAreaElement>) => setDraft(event.target.value),
    [],
  );

  const handleComposerKeyDown = useCallback(
    (event: KeyboardEvent<HTMLTextAreaElement>) => {
      if (event.key === "Enter" && !event.shiftKey) {
        event.preventDefault();
        send();
      }
    },
    [send],
  );

  const handleMessageAction = useCallback(
    (action: MessageAction, message: DesignMessage) => {
      if (action === "stop" && message.role === "assistant") {
        const activeGeneration = activeGenerationRef.current;
        if (activeGeneration === null || activeGeneration.delivered) return;

        activeGeneration.controller.abort();
        activeGenerationRef.current = null;
        documentRevisionRef.current += 1;
        setBusy(false);
        setMessages((current) =>
          current.map((item) =>
            item.id === message.id && item.role === "assistant"
              ? {
                  ...item,
                  status: "done",
                  title: "Stopped",
                  desc: "Cancelled before the host returned a result.",
                }
              : item,
          ),
        );
        return;
      }

      if (action === "select") {
        const nodeId = message.role === "assistant" ? message.nodeIds[0] : null;
        if (nodeId) selectLayer(nodeId);
        else startGeneration("Run a visual check on the canvas.");
        return;
      }

      if (action === "retry") {
        const prompt = promptForMessage(messages, message);
        if (prompt !== null) startGeneration(prompt);
        return;
      }

      const prompt = promptForMessage(messages, message);
      if (prompt !== null) startGeneration(prompt);
    },
    [messages, selectLayer, startGeneration],
  );

  const clearComposerContext = useCallback(() => setComposerContextLayerId(null), []);

  return (
    <section
      ref={designSurfaceRef}
      className="surface-card design-surface"
      data-screen-label="Design"
      aria-labelledby="design-surface-title"
    >
      <h1 className="design-sr-only" id="design-surface-title">
        Design
      </h1>
      {disclosure ? <div className="design-demo-disclosure">{disclosure}</div> : null}
      <DesignToolbar
        documentName={document.name}
        documentPath={document.path}
        grounded={grounded}
        canSave={canSave}
        saved={saved}
        saving={saving}
        saveError={saveError}
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
            content={document.canvasContent}
            layers={layers}
            hiddenLayerIds={snapshot.hiddenLayerIds}
            pan={pan}
            selectedLayerId={selectedLayerId}
            tool={tool}
            zoom={zoom}
            onSelectLayer={selectLayer}
            onViewportChange={setViewport}
          />
          <LayerPanel
            layers={layerRows}
            onSelect={selectLayer}
            onToggleVisibility={toggleLayerVisibility}
          />
          <div className="design-tool-controls" aria-label="Canvas tools">
            <button
              type="button"
              title="Move / select"
              aria-pressed={tool === "move"}
              className={tool === "move" ? "design-tool-selected" : ""}
              onClick={setMoveTool}
            >
              Move
            </button>
            <button
              type="button"
              title="Drag a region, then let the AI analyze and fix it"
              aria-pressed={tool === "ai"}
              className={tool === "ai" ? "design-tool-selected" : ""}
              onClick={setAiTool}
            >
              Spot Edit
            </button>
          </div>
          <ZoomControls
            zoom={zoom}
            canZoomIn={zoom < DESIGN_MAX_ZOOM}
            canZoomOut={zoom > DESIGN_MIN_ZOOM}
            onZoomIn={zoomIn}
            onZoomOut={zoomOut}
            onZoomReset={zoomReset}
            onFit={fitCanvas}
          />
          <InspectorPanel
            layer={selectedLayer}
            tokenFooter={document.tokenFooter}
            radiusOptions={radiusOptions}
            flat={snapshot.flat}
            onRadiusChange={setRadius}
            onElevationChange={setElevation}
            onDuplicate={duplicateLayer}
            onDelete={deleteLayer}
            canDelete={layers.length > 1}
          />
        </div>

        <DesignAssistant
          canGenerate={canGenerate}
          contextPrefix={document.contextPrefix}
          generationLabel={generationLabel}
          contextLayerName={composerContextLayerName}
          provider={document.provider}
          draft={draft}
          draftPlaceholder={
            composerContextLayerName ? document.draftPlaceholder : document.noContextPlaceholder
          }
          sendLabel={busy ? "Working…" : "Generate"}
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
