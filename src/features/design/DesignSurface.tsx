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
import { findUndefinedCustomProperties } from "./artifactTokenLint";
import { AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS } from "./agentHost";
import {
  builtInSkillIndex,
  builtInSkillSources,
  type BuiltInSkillIndexEntry,
} from "./builtInSkills";
import {
  DEFAULT_DESIGN_SKILL_SELECTION,
  loadDesignSkillSelection,
  saveDesignSkillSelection,
  selectedSlugs,
  type DesignSkillSelection,
} from "./designSettings";
import { buildSkillBlock } from "./skillLoader";
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
  layerNotice?: string;
  artifactHtml?: string;
  artifactError?: string;
  artifactMissingTokens: readonly string[];
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
  canDuplicate: boolean;
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
  messages: readonly DesignMessage[];
  assistantRef: RefObject<HTMLDivElement | null>;
  onDraftChange: (event: ChangeEvent<HTMLTextAreaElement>) => void;
  onComposerKeyDown: (event: KeyboardEvent<HTMLTextAreaElement>) => void;
  onSend: () => void;
  onVisualCheck: () => void;
  onClearContext: () => void;
  onMessageAction: (action: MessageAction, message: DesignMessage) => void;
  skillIndex: readonly BuiltInSkillIndexEntry[];
  skillSelection: DesignSkillSelection;
  selectedSkillSlugs: readonly string[];
  autoAppliedSkillSlugs: readonly string[] | null;
  autoSkillNotice: string | null;
  onSkillModeChange: (mode: DesignSkillSelection["mode"]) => void;
  onSkillToggle: (slug: string) => void;
}

type SnapshotChange = (current: DesignSnapshot) => DesignSnapshot | null;

function cloneMessages(document: DesignDocument): DesignMessage[] {
  return cloneMessageList(document.messages).map((message) =>
    normalizeIncompleteMessage(message, "loaded"),
  );
}

function cloneMessageList(messages: readonly DesignMessage[]): DesignMessage[] {
  return messages.map((message) =>
    message.role === "user"
      ? { ...message }
      : { ...message, sources: [...message.sources], nodeIds: [...message.nodeIds] },
  );
}

function normalizeIncompleteMessage(
  message: DesignMessage,
  phase: "loaded" | "saved",
): DesignMessage {
  if (message.role !== "assistant" || message.status !== "working") return message;
  const boundary = phase === "saved" ? "saved" : "loaded";
  return {
    ...message,
    status: "error",
    title: "Generation incomplete",
    desc: `This generation did not complete before the document was ${boundary}.`,
  };
}

function terminalMessagesForSave(messages: readonly DesignMessage[]): DesignMessage[] {
  return cloneMessageList(messages).map((message) => normalizeIncompleteMessage(message, "saved"));
}

function messageActions(message: DesignMessage, canGenerate: boolean): readonly MessageAction[] {
  if (message.role === "user") return [];
  if (message.status === "working") return canGenerate ? ["stop"] : [];
  if (message.status === "error") return canGenerate ? ["retry"] : [];
  if (message.nodeIds.length === 0) return canGenerate ? ["regenerate"] : [];
  return canGenerate ? ["select", "regenerate"] : ["select"];
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
        {layer.source ? (
          <div className="design-node-actions">
            <span className="design-node-primary-action" title={layer.source.path}>
              {sourceDirectory(layer.source.path)}
            </span>
          </div>
        ) : null}
      </div>
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

const DESIGN_GRID_ORIGIN_X = 60;
const DESIGN_GRID_ORIGIN_Y = 46;
const ARTIFACT_NODE_WIDTH = 700;
const ARTIFACT_NODE_HEIGHT = 500;
const ARTIFACT_NODE_GAP = 32;
const ARTIFACT_NODE_ID = "generated-artifact";
const ARTIFACT_CONTEXT_NAME = "Generated artifact";
const ARTIFACT_CSP =
  "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; manifest-src 'none'";
const ARTIFACT_CSP_META = `<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_CSP}" />`;

function layerRectsFor(layers: readonly DesignLayer[]): NodeRect[] {
  return layers.map((layer, index) => ({
    id: layer.id,
    x: layer.transform.x,
    y: layer.transform.y,
    w: layer.transform.width,
    h: layer.transform.height,
    z: index,
  }));
}

function artifactNodeRect(layers: readonly DesignLayer[]): NodeRect {
  const bounds = nodesBounds(layerRectsFor(layers));
  return {
    id: ARTIFACT_NODE_ID,
    x: bounds?.x ?? DESIGN_GRID_ORIGIN_X,
    y: bounds === null ? DESIGN_GRID_ORIGIN_Y : bounds.y + bounds.h + ARTIFACT_NODE_GAP,
    w: ARTIFACT_NODE_WIDTH,
    h: ARTIFACT_NODE_HEIGHT,
    z: layers.length,
  };
}

function sourceDirectory(path: string): string {
  const separator = path.lastIndexOf("/");
  return separator > 0 ? path.slice(0, separator) : ".";
}

function artifactSrcDoc(html: string): string {
  return `${ARTIFACT_CSP_META}\n${html}`;
}

function latestArtifact(messages: readonly DesignMessage[]): {
  html?: string;
  error?: string;
} | null {
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index];
    if (
      message.role === "assistant" &&
      message.status === "done" &&
      (message.artifactHtml !== undefined || message.artifactError !== undefined)
    ) {
      return { html: message.artifactHtml, error: message.artifactError };
    }
  }
  return null;
}

const DesignCanvas = memo(function DesignCanvas({
  content,
  layers,
  hiddenLayerIds,
  pan,
  selectedLayerId,
  tool,
  zoom,
  layerNotice,
  artifactHtml,
  artifactError,
  artifactMissingTokens,
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
    () => layerRectsFor(layers).filter((layer) => !hiddenLayerIds.includes(layer.id)),
    [hiddenLayerIds, layers],
  );
  const artifactRect = useMemo(
    () =>
      artifactHtml !== undefined || artifactError !== undefined ? artifactNodeRect(layers) : null,
    [artifactError, artifactHtml, layers],
  );
  const hitRects = useMemo<NodeRect[]>(
    () => (artifactRect === null ? layerRects : [...layerRects, artifactRect]),
    [artifactRect, layerRects],
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
      let target = hitTest(point, hitRects);
      if (!target && event.target instanceof Element) {
        const clickedNode = event.target.closest<HTMLElement>("[data-canvas-layer-id]");
        const clickedRect = hitRects.find(
          (layer) => layer.id === clickedNode?.dataset.canvasLayerId,
        );
        if (clickedRect) {
          onSelectLayer(clickedRect.id);
          return;
        }
      }
      onSelectLayer(target?.id ?? "");
    },
    [hitRects, onSelectLayer],
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
      {layerNotice && layers.length > 0 ? (
        <div
          className="design-canvas-notice"
          role="status"
          style={{ pointerEvents: "none", zIndex: 1 }}
        >
          {layerNotice}
        </div>
      ) : null}
      {layers.length === 0 ? (
        <div className="design-canvas-empty" role="status">
          {layerNotice ?? "No design components found."}
        </div>
      ) : null}
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
        {artifactRect !== null ? (
          <div
            className={`design-canvas-artifact${selectedLayerId === ARTIFACT_NODE_ID ? " design-canvas-artifact-selected" : ""}`}
            style={{
              left: artifactRect.x,
              top: artifactRect.y,
              width: artifactRect.w,
              height: artifactRect.h,
            }}
            data-canvas-layer-id={ARTIFACT_NODE_ID}
            role="button"
            tabIndex={0}
            aria-label="Select generated artifact"
            aria-pressed={selectedLayerId === ARTIFACT_NODE_ID}
            onClick={() => onSelectLayer(ARTIFACT_NODE_ID)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onSelectLayer(ARTIFACT_NODE_ID);
              }
            }}
          >
            {artifactError ? (
              <div className="design-canvas-artifact-error" role="status">
                {artifactError}
              </div>
            ) : (
              <div className="design-canvas-artifact-content" inert>
                {/*
                  WebView2 measurement on 2026-09-05: the parent CSP is not inherited by srcdoc.
                  This policy is therefore delivered inside the frame; the sandbox remains a
                  separate boundary, and later artifact policies cannot relax this one.
                */}
                <iframe
                  sandbox=""
                  srcDoc={artifactSrcDoc(artifactHtml ?? "")}
                  title="Generated artifact"
                  className="design-artifact-frame"
                  style={{ pointerEvents: "none" }}
                />
              </div>
            )}
            {artifactMissingTokens.length > 0 ? (
              <div className="design-canvas-artifact-token-warning" role="status">
                This artifact references {artifactMissingTokens.length === 1 ? "a token" : "tokens"}{" "}
                it does not define: {artifactMissingTokens.join(", ")}.
              </div>
            ) : null}
          </div>
        ) : null}
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
  canDuplicate,
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
        {canDuplicate ? (
          <button type="button" onClick={onDuplicate}>
            Duplicate
          </button>
        ) : null}
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
  canGenerate,
  message,
  onAction,
}: {
  canGenerate: boolean;
  message: DesignMessage;
  onAction: (action: MessageAction, message: DesignMessage) => void;
}) {
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
          className={`design-message-icon design-message-icon-${message.status === "working" ? "working" : message.status === "error" ? "error" : "done"}`}
          aria-hidden="true"
        >
          {message.status === "working" ? "◌" : message.status === "error" ? "!" : "✓"}
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
        {messageActions(message, canGenerate).map((action) => (
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
  skillIndex,
  skillSelection,
  selectedSkillSlugs,
  autoAppliedSkillSlugs,
  autoSkillNotice,
  onSkillModeChange,
  onSkillToggle,
}: AssistantProps) {
  const [skillPickerOpen, setSkillPickerOpen] = useState(false);
  const selectedSlugSet = useMemo(() => new Set(selectedSkillSlugs), [selectedSkillSlugs]);
  const resolvedSkillSlugs =
    skillSelection.mode === "auto" ? autoAppliedSkillSlugs : selectedSkillSlugs;
  const hasResolvedComposition =
    skillSelection.mode === "all" ||
    (skillSelection.mode === "auto" && autoAppliedSkillSlugs !== null);
  const skillBlock = useMemo(
    () => buildSkillBlock(builtInSkillSources(), resolvedSkillSlugs ?? []),
    [resolvedSkillSlugs],
  );
  const resolvedSkillSlugSet = useMemo(
    () => new Set(resolvedSkillSlugs ?? []),
    [resolvedSkillSlugs],
  );
  const automaticBaselineSlugSet = useMemo(
    () => new Set<string>(AUTOMATIC_ALWAYS_INCLUDED_SKILL_SLUGS),
    [],
  );
  const droppedSkillSlugSet = useMemo(() => new Set(skillBlock.dropped), [skillBlock]);
  const skillSummary =
    skillSelection.mode === "auto"
      ? "Craft: automatic"
      : skillSelection.mode === "all"
        ? "Craft: priority sections that fit"
        : selectedSkillSlugs.length === 0
          ? `Craft: 0 of ${skillIndex.length} · no design guidance`
          : `Craft: ${selectedSkillSlugs.length} of ${skillIndex.length}`;
  const skillPreview = useMemo(() => skillBlock.text, [skillBlock]);

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
        {messages.map((message) => (
          <DesignMessageCard
            key={message.id}
            canGenerate={canGenerate}
            message={message}
            onAction={onMessageAction}
          />
        ))}
      </div>

      {canGenerate ? (
        <div className="design-composer-wrap">
          <div className="design-composer-meta">
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
            <div className="design-skill-controls">
              <button
                className="design-skill-summary"
                type="button"
                aria-expanded={skillPickerOpen}
                aria-controls="design-skill-picker"
                aria-label="Configure design craft"
                onClick={() => setSkillPickerOpen((open) => !open)}
              >
                {skillSummary}
              </button>
              {autoSkillNotice ? (
                <div className="design-skill-result" role="status">
                  {autoSkillNotice}
                </div>
              ) : null}
              {skillPickerOpen ? (
                <div
                  id="design-skill-picker"
                  className="design-skill-picker"
                  role="group"
                  aria-label="Design craft sections"
                >
                  <p className="design-skill-purpose">
                    These sections are added to every design request, so the agent works to the same
                    standards each time.
                  </p>
                  <fieldset className="design-skill-modes">
                    <legend>Apply craft sections</legend>
                    <label>
                      <input
                        type="radio"
                        name="design-skill-mode"
                        value="all"
                        checked={skillSelection.mode === "all"}
                        onChange={() => onSkillModeChange("all")}
                      />
                      <span>Priority</span>
                      <small>Most important sections that fit; the rest are omitted.</small>
                    </label>
                    <label>
                      <input
                        type="radio"
                        name="design-skill-mode"
                        value="manual"
                        checked={skillSelection.mode === "manual"}
                        onChange={() => onSkillModeChange("manual")}
                      />
                      <span>Manual</span>
                      <small>Exactly the sections you tick.</small>
                    </label>
                    <label>
                      <input
                        type="radio"
                        name="design-skill-mode"
                        value="auto"
                        checked={skillSelection.mode === "auto"}
                        onChange={() => onSkillModeChange("auto")}
                      />
                      <span>Automatic</span>
                      <small>The agent chooses relevant sections for each request.</small>
                    </label>
                  </fieldset>
                  <div className="design-skill-list">
                    {skillIndex.map((entry) => {
                      const isAutomaticBaseline =
                        skillSelection.mode === "auto" && automaticBaselineSlugSet.has(entry.slug);
                      const isRequested = resolvedSkillSlugSet.has(entry.slug);
                      const isDropped =
                        hasResolvedComposition &&
                        isRequested &&
                        droppedSkillSlugSet.has(entry.slug);
                      const isIncluded =
                        !isDropped &&
                        ((hasResolvedComposition && isRequested) || isAutomaticBaseline);
                      const isAutomaticallyUnselected =
                        skillSelection.mode === "auto" &&
                        autoAppliedSkillSlugs !== null &&
                        !isRequested &&
                        !isAutomaticBaseline;
                      const status = isDropped
                        ? `Omitted: did not fit within the ${skillBlock.ceiling.toLocaleString()}-character budget.`
                        : isAutomaticBaseline
                          ? "Always included automatically."
                          : isAutomaticallyUnselected
                            ? "Not chosen automatically."
                            : null;
                      const rowClass = [
                        "design-skill-option",
                        skillSelection.mode !== "manual" ? "design-skill-option-locked" : null,
                        isIncluded ? "design-skill-option-included" : null,
                        isDropped ? "design-skill-option-dropped" : null,
                        isAutomaticBaseline ? "design-skill-option-always-included" : null,
                        isAutomaticallyUnselected ? "design-skill-option-not-selected" : null,
                      ]
                        .filter((className): className is string => className !== null)
                        .join(" ");

                      return (
                        <label className={rowClass} key={entry.slug}>
                          <input
                            type="checkbox"
                            aria-label={`Apply ${entry.title}`}
                            // Automatic has not decided yet, and an empty box would say it
                            // decided no.  `indeterminate` is the state HTML already has for
                            // exactly this, announced as "mixed" rather than "not checked".
                            ref={(node) => {
                              if (node !== null) {
                                node.indeterminate =
                                  skillSelection.mode === "auto" &&
                                  autoAppliedSkillSlugs === null &&
                                  !isAutomaticBaseline;
                              }
                            }}
                            checked={
                              skillSelection.mode === "manual"
                                ? selectedSlugSet.has(entry.slug)
                                : isIncluded
                            }
                            disabled={skillSelection.mode !== "manual"}
                            onChange={() => onSkillToggle(entry.slug)}
                          />
                          <span className="design-skill-option-copy">
                            <span className="design-skill-option-title">{entry.title}</span>
                            <span className="design-skill-option-description">
                              {entry.description}
                            </span>
                            {status !== null ? (
                              <span className="design-skill-option-status">{status}</span>
                            ) : null}
                          </span>
                        </label>
                      );
                    })}
                  </div>
                  {skillPreview.length > 0 ? (
                    <details className="design-skill-preview">
                      <summary>What the agent will be told</summary>
                      <pre>{skillPreview}</pre>
                    </details>
                  ) : null}
                </div>
              ) : null}
            </div>
          </div>
          <div className="design-composer">
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
    const controller = new AbortController();
    void host
      .loadDocument(controller.signal)
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
      controller.abort();
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
  const skillIndex = useMemo(() => builtInSkillIndex(), []);
  const knownSkillSlugs = useMemo(() => skillIndex.map((entry) => entry.slug), [skillIndex]);
  const initialSnapshot: DesignSnapshot = {
    hiddenLayerIds: document.initialState.hiddenLayerIds,
    layers: cloneLayers(document),
    radius: document.initialState.radius,
    flat: document.initialState.flat,
  };
  const initialViewState: DesignViewState = {
    pan: { x: 0, y: 0 },
    selectedLayerId: document.selectedLayerId,
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
  const [grounded, setGrounded] = useState(document.grounded);
  const [draft, setDraft] = useState(document.initialState.draft);
  const [busy, setBusy] = useState(false);
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [messages, setMessages] = useState<DesignMessage[]>(() => cloneMessages(document));
  const [skillSelection, setSkillSelectionState] = useState<DesignSkillSelection>(
    DEFAULT_DESIGN_SKILL_SELECTION,
  );
  const [autoAppliedSkillSlugs, setAutoAppliedSkillSlugs] = useState<readonly string[] | null>(
    null,
  );
  const [autoSkillNotice, setAutoSkillNotice] = useState<string | null>(null);

  const savingRef = useRef(false);
  const mountedRef = useRef(true);
  const activeGenerationRef = useRef<{
    controller: AbortController;
    delivered: boolean;
  } | null>(null);
  const messagesRef = useRef(messages);
  const documentRevisionRef = useRef(0);
  const layerCopyCounterRef = useRef(0);
  const skillSelectionInteractedRef = useRef(false);
  const assistantRef = useRef<HTMLDivElement>(null);
  const designSurfaceRef = useRef<HTMLElement>(null);

  const snapshot = history.present;
  const layers = snapshot.layers;
  const saved = history.saved;
  const { pan, selectedLayerId, tool, zoom } = viewState;

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      activeGenerationRef.current?.controller.abort();
    };
  }, []);

  useEffect(() => {
    messagesRef.current = messages;
    if (assistantRef.current) assistantRef.current.scrollTop = assistantRef.current.scrollHeight;
  }, [messages, busy]);

  useEffect(() => {
    let active = true;
    void loadDesignSkillSelection(knownSkillSlugs).then((selection) => {
      if (active && !skillSelectionInteractedRef.current) setSkillSelectionState(selection);
    });
    return () => {
      active = false;
    };
  }, [knownSkillSlugs]);

  const selectedLayer = useMemo(
    () => layers.find((layer) => layer.id === selectedLayerId) ?? null,
    [layers, selectedLayerId],
  );

  // The same resolved list drives both the composer summary and each generation.
  const selectedSkillSlugs = useMemo(
    () => selectedSlugs(skillSelection, knownSkillSlugs),
    [knownSkillSlugs, skillSelection],
  );
  const updateSkillSelection = useCallback((selection: DesignSkillSelection) => {
    skillSelectionInteractedRef.current = true;
    setSkillSelectionState(selection);
    void saveDesignSkillSelection(selection);
  }, []);
  const handleSkillModeChange = useCallback(
    (mode: DesignSkillSelection["mode"]) => {
      if (skillSelection.mode === mode) return;
      setAutoSkillNotice(null);
      setAutoAppliedSkillSlugs(null);
      updateSkillSelection({ ...skillSelection, mode });
    },
    [skillSelection, updateSkillSelection],
  );
  const handleSkillToggle = useCallback(
    (slug: string) => {
      if (skillSelection.mode !== "manual") return;
      const enabled = new Set(skillSelection.enabledSlugs);
      if (enabled.has(slug)) enabled.delete(slug);
      else enabled.add(slug);
      updateSkillSelection({
        ...skillSelection,
        enabledSlugs: [...enabled],
      });
    },
    [skillSelection, updateSkillSelection],
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

  const artifact = latestArtifact(messages);
  const artifactHtml = artifact?.html;
  const artifactError = artifact?.error;
  const artifactMissingTokens = useMemo(
    () =>
      artifactHtml !== undefined && artifactError === undefined
        ? findUndefinedCustomProperties(artifactHtml)
        : [],
    [artifactError, artifactHtml],
  );
  const artifactRect = useMemo(
    () =>
      artifactHtml !== undefined || artifactError !== undefined ? artifactNodeRect(layers) : null,
    [artifactError, artifactHtml, layers],
  );

  const fitRects = useMemo<NodeRect[]>(() => {
    const rects = layerRectsFor(layers).filter(
      (layer) => !snapshot.hiddenLayerIds.includes(layer.id),
    );
    return artifactRect === null ? rects : [...rects, artifactRect];
  }, [artifactRect, layers, snapshot.hiddenLayerIds]);
  const fitRectsRef = useRef<NodeRect[]>([]);
  useEffect(() => {
    fitRectsRef.current = fitRects;
  }, [fitRects]);

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

  const generationCount = useMemo(
    () =>
      messages.filter(
        (message): message is DesignAssistantMessage =>
          message.role === "assistant" && message.status === "done",
      ).length,
    [messages],
  );

  const generationLabel = `${generationCount} ${generationCount === 1 ? "generation" : "generations"}`;
  const composerContextTarget = useMemo(() => {
    if (
      composerContextLayerId === ARTIFACT_NODE_ID &&
      (artifactHtml !== undefined || artifactError !== undefined)
    ) {
      return {
        label: ARTIFACT_CONTEXT_NAME,
        scope: `${document.contextPrefix} ${ARTIFACT_CONTEXT_NAME}; the user is refining the artifact the agent just produced.`,
      };
    }

    const layer = layers.find((candidate) => candidate.id === composerContextLayerId);
    if (!layer) return null;
    const sourcePath = layer.source ? `; source file: ${layer.source.path}` : "";
    return {
      label: layer.name,
      scope: `${document.contextPrefix} ${layer.name} (${layer.kind})${sourcePath}; the user is pointing at the layer named "${layer.name}".`,
    };
  }, [artifactError, artifactHtml, composerContextLayerId, document.contextPrefix, layers]);
  const composerContextLayerName = composerContextTarget?.label ?? null;
  const canUndo = history.past.length > 0;
  const canRedo = history.future.length > 0;

  const commitSnapshot = useCallback((change: SnapshotChange) => {
    documentRevisionRef.current += 1;
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

  const markDocumentDirty = useCallback(() => {
    documentRevisionRef.current += 1;
    setHistory((current) => (current.saved ? { ...current, saved: false } : current));
  }, []);

  const selectLayer = useCallback(
    (layerId: string) => {
      const selectionChanged = selectedLayerId !== layerId || composerContextLayerId !== layerId;
      if (selectionChanged) markDocumentDirty();
      setViewState((current) =>
        current.selectedLayerId === layerId ? current : { ...current, selectedLayerId: layerId },
      );
      setComposerContextLayerId((current) => (current === layerId ? current : layerId));
    },
    [composerContextLayerId, markDocumentDirty, selectedLayerId],
  );

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
    if (layers.length <= 1 || selectedLayer === null) return;

    const selectedIndex = layers.findIndex((layer) => layer.id === selectedLayerId);
    const nextLayer = layers[selectedIndex + 1] ?? layers[selectedIndex - 1];
    if (!nextLayer) return;

    selectLayer(nextLayer.id);
    commitSnapshot((current) => ({
      ...current,
      layers: current.layers.filter((layer) => layer.id !== selectedLayerId),
      hiddenLayerIds: current.hiddenLayerIds.filter((id) => id !== selectedLayerId),
    }));
  }, [commitSnapshot, layers, selectedLayer, selectedLayerId, selectLayer]);

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
    setViewport(fitViewport(nodesBounds(fitRectsRef.current), bounds.width, bounds.height));
  }, [setViewport]);

  const undo = useCallback(() => {
    if (!canUndo) return;
    documentRevisionRef.current += 1;
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
    documentRevisionRef.current += 1;
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
    if (saveDocument === undefined || savingRef.current) return;

    savingRef.current = true;
    const revisionAtSave = documentRevisionRef.current;
    const hasWorkingMessage = messages.some(
      (message) => message.role === "assistant" && message.status === "working",
    );
    const documentToSave: DesignDocument = {
      ...document,
      initialState: {
        ...document.initialState,
        hiddenLayerIds: [...history.present.hiddenLayerIds],
        radius: history.present.radius,
        flat: history.present.flat,
      },
      selectedLayerId,
      grounded,
      layers: cloneLayerList(layers),
      messages: terminalMessagesForSave(messages),
    };

    setSaveError(null);
    setSaving(true);
    try {
      await saveDocument(documentToSave);
      if (documentRevisionRef.current === revisionAtSave && !hasWorkingMessage) {
        if (mountedRef.current) {
          setHistory((current) => (current.saved ? current : { ...current, saved: true }));
        }
      }
    } catch (error: unknown) {
      if (mountedRef.current) {
        setHistory((current) => (current.saved ? { ...current, saved: false } : current));
        setSaveError(
          error instanceof Error ? error.message : "The design document could not save.",
        );
      }
    } finally {
      savingRef.current = false;
      if (mountedRef.current) setSaving(false);
    }
  }, [document, grounded, history.present, layers, messages, saveDocument, selectedLayerId]);
  const toggleGrounding = useCallback(() => {
    markDocumentDirty();
    setGrounded((value) => !value);
  }, [markDocumentDirty]);

  const startGeneration = useCallback(
    (prompt: string) => {
      if (busy || generate === undefined) return;
      const scopedPrompt = composerContextTarget
        ? `${prompt}\n\nScope: ${composerContextTarget.scope}`
        : prompt;
      activeGenerationRef.current?.controller.abort();
      const controller = new AbortController();
      const activeGeneration = { controller, delivered: false };
      const userId = crypto.randomUUID();
      const assistantId = crypto.randomUUID();
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
      setAutoSkillNotice(null);
      setAutoAppliedSkillSlugs(null);
      const generationOptions =
        skillSelection.mode === "auto"
          ? { skillMode: "auto" as const }
          : { skills: selectedSkillSlugs };
      void generate(scopedPrompt, controller.signal, generationOptions)
        .then((result) => {
          if (controller.signal.aborted || activeGenerationRef.current !== activeGeneration) return;
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
                    artifactHtml: result.artifactHtml,
                    artifactError: result.artifactError,
                    instruction: prompt,
                  }
                : message,
            ),
          );
          if (skillSelection.mode === "auto" && result.appliedSkillSlugs !== undefined) {
            const composedAutoSkillBlock = buildSkillBlock(
              builtInSkillSources(),
              result.appliedSkillSlugs,
            );
            const droppedAutoSkillSlugs = new Set(composedAutoSkillBlock.dropped);
            const appliedTitles = result.appliedSkillSlugs
              .filter((slug) => !droppedAutoSkillSlugs.has(slug))
              .map((slug) => skillIndex.find((entry) => entry.slug === slug)?.title)
              .filter((title): title is string => title !== undefined);
            const droppedTitles = result.appliedSkillSlugs
              .filter((slug) => droppedAutoSkillSlugs.has(slug))
              .map((slug) => skillIndex.find((entry) => entry.slug === slug)?.title)
              .filter((title): title is string => title !== undefined);
            setAutoAppliedSkillSlugs([...result.appliedSkillSlugs]);
            setAutoSkillNotice(
              result.skillSelectionFallback
                ? "Automatic choice did not happen; the most important sections that fit were used, and the rest were omitted."
                : appliedTitles.length > 0
                  ? `Automatic craft: ${appliedTitles.join(", ")}${
                      droppedTitles.length > 0
                        ? `. Omitted: ${droppedTitles.join(", ")} did not fit within the ${composedAutoSkillBlock.ceiling.toLocaleString()}-character budget.`
                        : ""
                    }`
                  : "Automatic craft: no sections were used.",
            );
          }
          setBusy(false);
          setHistory((current) => (current.saved ? { ...current, saved: false } : current));
          activeGenerationRef.current = null;
        })
        .catch((error: unknown) => {
          if (controller.signal.aborted || activeGenerationRef.current !== activeGeneration) return;
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
    [
      busy,
      composerContextLayerName,
      composerContextTarget,
      document.contextPrefix,
      document.workingMessage,
      generate,
      skillIndex,
      skillSelection.mode,
      selectedSkillSlugs,
    ],
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

        activeGenerationRef.current = null;
        activeGeneration.controller.abort();
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
        return;
      }

      if (action === "retry") {
        const prompt = promptForMessage(messagesRef.current, message);
        if (prompt !== null) startGeneration(prompt);
        return;
      }

      const prompt = promptForMessage(messagesRef.current, message);
      if (prompt !== null) startGeneration(prompt);
    },
    [selectLayer, startGeneration],
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
            layerNotice={document.layerNotice}
            artifactHtml={artifactHtml}
            artifactError={artifactError}
            artifactMissingTokens={artifactMissingTokens}
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
          {selectedLayer ? (
            <InspectorPanel
              layer={selectedLayer}
              tokenFooter={document.tokenFooter}
              radiusOptions={radiusOptions}
              flat={snapshot.flat}
              onRadiusChange={setRadius}
              onElevationChange={setElevation}
              onDuplicate={duplicateLayer}
              onDelete={deleteLayer}
              canDuplicate={selectedLayer.source === undefined}
              canDelete={layers.length > 1}
            />
          ) : null}
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
          messages={messages}
          assistantRef={assistantRef}
          onDraftChange={handleDraftChange}
          onComposerKeyDown={handleComposerKeyDown}
          onSend={send}
          onVisualCheck={visualCheck}
          onClearContext={clearComposerContext}
          onMessageAction={handleMessageAction}
          skillIndex={skillIndex}
          skillSelection={skillSelection}
          selectedSkillSlugs={selectedSkillSlugs}
          autoAppliedSkillSlugs={autoAppliedSkillSlugs}
          autoSkillNotice={autoSkillNotice}
          onSkillModeChange={handleSkillModeChange}
          onSkillToggle={handleSkillToggle}
        />
      </div>
    </section>
  );
}
