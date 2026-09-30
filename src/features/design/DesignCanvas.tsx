import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type {
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
  ReactNode,
} from "react";
import { artifactSrcDoc } from "./artifactCsp";
import { ArtifactRenderCritic, type ArtifactRenderCriticResult } from "./artifactRenderCritic";
import {
  clampArtifactScroll,
  maxArtifactScroll,
  revealArtifactRect,
  scrollArtifactBy,
} from "./artifactViewport";
import type { DesignLayer } from "./designHost";
import type { NodeRect } from "../../types/geometry";
import { hitTest } from "../../lib/canvas/hitTest";
import type { Pan } from "../../lib/canvas/viewportMath";
import {
  ARTIFACT_NODE_ID,
  artifactNodeRect,
  layerRectsFor,
  smallestSectionAt,
} from "./designCanvasGeometry";
import { isHidden } from "./designLayerTree";
import {
  createViewport,
  createViewportCommitScheduler,
  panViewport,
  pointerToWorld,
  viewportTransform,
  zoomViewport,
  type DesignViewport,
} from "./designViewport";

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
  /**
   * The pill's trailing actions, or nothing. Opaque on purpose: the zoom
   * controls own the container, never the artifact the actions act on.
   */
  children?: ReactNode;
}

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

export const ZoomControls = memo(function ZoomControls({
  zoom,
  canZoomIn,
  canZoomOut,
  onZoomIn,
  onZoomOut,
  onZoomReset,
  onFit,
  children,
}: ZoomControlsProps) {
  const zoomLabel = `${Math.round(zoom * 100)}%`;

  return (
    <div className="design-zoom-controls" aria-label="Canvas controls">
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
      {children}
    </div>
  );
});

function sourceDirectory(path: string): string {
  const separator = path.lastIndexOf("/");
  return separator > 0 ? path.slice(0, separator) : ".";
}

interface CanvasProps {
  layers: readonly DesignLayer[];
  /** Measured page sections living inside the artifact frame. */
  sectionLayers: readonly DesignLayer[];
  hiddenLayerIds: readonly string[];
  pan: Pan;
  selectedLayerId: string;
  zoom: number;
  layerNotice?: string;
  artifactHtml?: string;
  artifactError?: string;
  artifactMissingTokens: readonly string[];
  /**
   * The slides-contract report for the artifact on screen, or `""` when there is
   * nothing to say: the artifact was not generated in slides mode, or it already
   * has the shape slides mode asked for. A notice, never a gate — the artifact
   * still renders, exports and copies whatever it says.
   */
  artifactSlideShapeNotice: string;
  /**
   * The report for an artifact whose reply carried more than one ```html block,
   * or `""` when there is nothing to say: the reply carried one block, or the
   * producing run recorded no count. The canvas shows the last block, so this
   * states how many were dropped rather than leaving them unaccounted for. A
   * notice, never a gate — the artifact renders, exports and copies regardless.
   */
  artifactFencedBlockNotice: string;
  artifactHeight: number;
  /**
   * Measured full page height in page CSS px, or undefined when the artifact
   * has not been measured yet. Undefined keeps the frame unscrollable (today's
   * behaviour), never a guess.
   */
  artifactContentHeight?: number;
  /** Page-space highlight for the selected page section, if it is one. */
  sectionHighlight: NodeRect | null;
  /** Page-space marks for sections carrying an agent note. */
  noteMarks: readonly NodeRect[];
  onSelectLayer: (layerId: string) => void;
  onViewportChange: (viewport: DesignViewport) => void;
  onArtifactMeasured: (html: string, result: ArtifactRenderCriticResult) => void;
}

interface ScrollCommitScheduler {
  schedule(offset: number): void;
  flush(offset?: number): void;
  cancel(): void;
}

/**
 * One state write per frame for the artifact window's scroll offset.
 *
 * Same shape and same frame wiring as `createViewportCommitScheduler`, which
 * does this job for the canvas viewport; that factory is typed to
 * `DesignViewport`, so a bare offset cannot travel through it. A trackpad emits
 * 60-120 wheel events a second, and each one used to write state, re-render the
 * canvas and reposition every section overlay on a page that can carry
 * hundreds. Only the last offset of a frame matters, so only the last is kept:
 * the write that reaches React is the cumulative offset, never a stale step.
 * `flush` is for the paths that must not wait for a frame (a selection reveal,
 * a new artifact) and it cancels the pending frame, so a queued wheel commit
 * cannot land on top of them.
 */
function createScrollCommitScheduler(
  commit: (offset: number) => void,
  scheduleFrame: (callback: () => void) => number,
  cancelFrame: (frameId: number) => void,
): ScrollCommitScheduler {
  let pending: number | null = null;
  let frameId: number | null = null;

  const commitPending = () => {
    frameId = null;
    const next = pending;
    pending = null;
    if (next !== null) commit(next);
  };

  return {
    schedule(offset) {
      pending = offset;
      if (frameId === null) frameId = scheduleFrame(commitPending);
    },
    flush(offset) {
      if (frameId !== null) cancelFrame(frameId);
      frameId = null;
      const next = offset ?? pending;
      pending = null;
      if (next !== null) commit(next);
    },
    cancel() {
      if (frameId !== null) cancelFrame(frameId);
      frameId = null;
      pending = null;
    },
  };
}

export const DesignCanvas = memo(function DesignCanvas({
  layers,
  sectionLayers,
  hiddenLayerIds,
  pan,
  selectedLayerId,
  zoom,
  layerNotice,
  artifactHtml,
  artifactError,
  artifactMissingTokens,
  artifactSlideShapeNotice,
  artifactFencedBlockNotice,
  artifactHeight,
  artifactContentHeight,
  sectionHighlight,
  noteMarks,
  onSelectLayer,
  onViewportChange,
  onArtifactMeasured,
}: CanvasProps) {
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

  // The artifact window's page-space scroll offset. It lives here, next to the
  // frame it moves, because the same number drives the iframe translate and the
  // parent-side section hit zones: one offset, so the two cannot drift apart.
  const [artifactScroll, setArtifactScroll] = useState(0);
  /**
   * The latest offset — committed, or still waiting for its frame. The wheel
   * accumulates against this rather than against the committed state, so two
   * events inside one frame compose instead of the second replacing the first
   * with a step measured from a value the first had already moved past.
   */
  const artifactScrollRef = useRef(0);
  const scrollCommitScheduler = useMemo(
    () =>
      createScrollCommitScheduler(
        setArtifactScroll,
        (callback) => window.requestAnimationFrame(callback),
        (frameId) => window.cancelAnimationFrame(frameId),
      ),
    [],
  );
  useEffect(() => () => scrollCommitScheduler.cancel(), [scrollCommitScheduler]);
  const artifactContentBoxHeight =
    artifactContentHeight === undefined
      ? undefined
      : Math.max(artifactHeight, artifactContentHeight);
  const artifactScrollOffset =
    artifactContentHeight === undefined
      ? 0
      : clampArtifactScroll(artifactScroll, artifactContentHeight, artifactHeight);
  /**
   * The one conversion from a section's page-space top to where it is drawn in
   * stage coordinates. A measured section's `transform` is page-space (its top
   * is the page's own, offset by the artifact's origin); the window scrolls that
   * page up by `artifactScrollOffset`, so every consumer — the overlay buttons,
   * the selection highlight, the hover highlight, the note marks, and the click
   * hit test — asks this function instead of subtracting on its own. Drawing and
   * picking therefore read the same number, which is what keeps a click on a
   * scrolled page from selecting the section that would sit there unscrolled.
   */
  const stageSectionTop = useCallback(
    (pageTop: number) => pageTop - artifactScrollOffset,
    [artifactScrollOffset],
  );
  const stageSectionLayers = useMemo(
    () =>
      sectionLayers.map((section) => ({
        ...section,
        transform: { ...section.transform, y: stageSectionTop(section.transform.y) },
      })),
    [sectionLayers, stageSectionTop],
  );
  const layerRects = useMemo<NodeRect[]>(
    () => layerRectsFor(layers).filter((layer) => !hiddenLayerIds.includes(layer.id)),
    [hiddenLayerIds, layers],
  );
  const artifactRect = useMemo(
    () =>
      artifactHtml !== undefined || artifactError !== undefined
        ? artifactNodeRect(layers, artifactHeight)
        : null,
    [artifactError, artifactHtml, artifactHeight, layers],
  );
  const hitRects = useMemo<NodeRect[]>(() => {
    const rects = artifactRect === null ? [...layerRects] : [...layerRects, artifactRect];
    // Sections sit above the artifact sheet so a click inside the frame
    // selects the section, not the whole page. Same z for all: last in
    // document order wins, which is the deepest element under the pointer.
    const sectionBase = artifactRect === null ? layerRects.length : artifactRect.z + 1;
    // Stage-space section rects: the same ones the overlays are drawn with, so a
    // click that falls through the section search below still cannot land on a
    // section by its unscrolled position.
    for (const section of stageSectionLayers) {
      if (hiddenLayerIds.includes(section.id)) continue;
      rects.push({
        id: section.id,
        x: section.transform.x,
        y: section.transform.y,
        w: section.transform.width,
        h: section.transform.height,
        z: sectionBase,
      });
    }
    return rects;
  }, [artifactRect, layerRects, stageSectionLayers, hiddenLayerIds]);

  // A new artifact is a new page: its window starts at the top. Committed at
  // once rather than scheduled, so a wheel commit still in flight cannot leave
  // the previous page's offset on the new one.
  useEffect(() => {
    artifactScrollRef.current = 0;
    scrollCommitScheduler.flush(0);
  }, [artifactHtml, artifactError, scrollCommitScheduler]);

  // A stale offset past the end of a re-measured page is harmless: the render,
  // the wheel, and the reveal all clamp against the current height.

  // Selecting a section must show it: the panel row and the canvas overlay both
  // land here, so the window scrolls to the section just chosen. A section
  // already inside the window returns the current offset, so nothing jumps.
  useEffect(() => {
    if (artifactRect === null || artifactContentHeight === undefined) return;
    const section = sectionLayers.find((layer) => layer.id === selectedLayerId);
    if (section === undefined) return;
    const next = revealArtifactRect(
      artifactScrollRef.current,
      { top: section.transform.y - artifactRect.y, height: section.transform.height },
      artifactContentHeight,
      artifactRect.h,
    );
    artifactScrollRef.current = next;
    // Flushed, not scheduled: a selection has to be on screen in the frame it
    // was made, and revealing a section that is already visible returns the
    // current offset, so nothing jumps.
    scrollCommitScheduler.flush(next);
  }, [artifactContentHeight, artifactRect, sectionLayers, selectedLayerId, scrollCommitScheduler]);

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
      // Sections first, smallest-wins (see smallestSectionAt): the overlay
      // buttons below already resolved the same way, so a click agrees with
      // a hover whatever path it arrived on. Both compare in stage space:
      // the point is world coordinates and the rects have the window's scroll
      // already applied, so they describe the same picture.
      const sectionHit = smallestSectionAt(stageSectionLayers, hiddenLayerIds, point);
      if (sectionHit !== null) {
        onSelectLayer(sectionHit.id);
        return;
      }
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
    [hitRects, hiddenLayerIds, onSelectLayer, stageSectionLayers],
  );

  // Direct-on-canvas hover: one id, cleared on leave. The highlight below
  // mirrors the selected-section highlight so hover and selection read as
  // the same affordance; the selected section keeps the solid style.
  const [hoveredSectionId, setHoveredSectionId] = useState<string | null>(null);
  const hoveredHighlight = useMemo(() => {
    if (hoveredSectionId === null || hoveredSectionId === selectedLayerId) return null;
    const hovered = stageSectionLayers.find((section) => section.id === hoveredSectionId);
    if (hovered === undefined || hiddenLayerIds.includes(hovered.id)) return null;
    return {
      x: hovered.transform.x,
      y: hovered.transform.y,
      w: hovered.transform.width,
      h: hovered.transform.height,
    };
  }, [hoveredSectionId, stageSectionLayers, selectedLayerId, hiddenLayerIds]);
  // Largest-first paint order: the smallest (deepest) overlay is on top and
  // receives the pointer, matching smallestSectionAt above.
  const sectionOverlays = useMemo(
    () =>
      [...stageSectionLayers]
        .filter((section) => !hiddenLayerIds.includes(section.id))
        .sort(
          (left, right) =>
            right.transform.width * right.transform.height -
            left.transform.width * left.transform.height,
        ),
    [hiddenLayerIds, stageSectionLayers],
  );

  const handleWheel = useCallback(
    (event: WheelEvent) => {
      const canvas = canvasRef.current;
      if (!canvas) return;
      const bounds = canvas.getBoundingClientRect();
      // Wheel is the established canvas zoom everywhere, including over the
      // artifact, so the page scroll gets its own modifier instead of taking
      // that over. Shift is the browser's unused "other axis" modifier; Ctrl is
      // reserved by the browser's page zoom and Alt can trip menus. Over an
      // artifact with room to scroll, Shift+wheel scrolls the window; anything
      // else (including a short page) falls through to zoom, so the gesture is
      // never dead.
      if (event.shiftKey && artifactRect !== null && artifactContentHeight !== undefined) {
        const point = pointerToWorld(
          event.clientX,
          event.clientY,
          { left: bounds.left, top: bounds.top },
          viewportRef.current,
        );
        const overArtifact =
          point.x >= artifactRect.x &&
          point.x <= artifactRect.x + artifactRect.w &&
          point.y >= artifactRect.y &&
          point.y <= artifactRect.y + artifactRect.h;
        if (overArtifact && maxArtifactScroll(artifactContentHeight, artifactHeight) > 0) {
          event.preventDefault();
          const next = scrollArtifactBy(
            artifactScrollRef.current,
            { deltaY: event.deltaY, deltaMode: event.deltaMode },
            artifactContentHeight,
            artifactHeight,
          );
          artifactScrollRef.current = next;
          // Scheduled, not written: the frame callback commits the last offset
          // of the burst, exactly like the zoom branch below commits its
          // viewport.
          scrollCommitScheduler.schedule(next);
          return;
        }
      }
      event.preventDefault();
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
    [
      applyViewport,
      artifactContentHeight,
      artifactHeight,
      artifactRect,
      scrollCommitScheduler,
      viewportCommitScheduler,
    ],
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
      tabIndex={-1}
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
      {layers.length === 0 && artifactRect === null ? (
        <div className="design-canvas-empty" role="status">
          {layerNotice === undefined ? (
            <>
              <p className="design-canvas-empty-title">The canvas is empty.</p>
              <p className="design-canvas-empty-copy">
                Describe the change you want in the composer, then choose Generate. The result
                appears here.
              </p>
            </>
          ) : (
            layerNotice
          )}
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
              <div
                className="design-canvas-artifact-content"
                inert
                style={
                  artifactContentBoxHeight === undefined
                    ? undefined
                    : {
                        height: `${artifactContentBoxHeight}px`,
                        // The page itself moves by the window's offset: its top
                        // is the page origin, so it goes through the same
                        // conversion the section rects above do.
                        transform: `translateY(${stageSectionTop(0)}px)`,
                      }
                }
              >
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
            {artifactMissingTokens.length > 0 || artifactHtml !== undefined ? (
              <div className="design-canvas-artifact-notices">
                {artifactMissingTokens.length > 0 ? (
                  <div className="design-canvas-artifact-token-warning" role="status">
                    This artifact references{" "}
                    {artifactMissingTokens.length === 1 ? "a token" : "tokens"} it does not define:{" "}
                    {artifactMissingTokens.join(", ")}.
                  </div>
                ) : null}
                {artifactSlideShapeNotice !== "" ? (
                  <div className="design-canvas-artifact-slide-notice" role="status">
                    {artifactSlideShapeNotice}
                  </div>
                ) : null}
                {artifactFencedBlockNotice !== "" ? (
                  <div className="design-canvas-artifact-fenced-block-notice" role="status">
                    {artifactFencedBlockNotice}
                  </div>
                ) : null}
                {artifactHtml !== undefined ? (
                  <ArtifactRenderCritic html={artifactHtml} onResult={onArtifactMeasured} />
                ) : null}
              </div>
            ) : null}
          </div>
        ) : null}
        {/*
          The section highlight is drawn by the parent OVER the closed iframe
          (like CanvasNode), never inside it: page rect + artifact origin, in
          world coordinates, minus the window's page scroll. The artifact box
          clips its own content but not this sibling, so a section below the
          fold still highlights at its true composed position under the sheet —
          declared, not hidden — and follows the offset the frame content moves
          by, so the box and the highlight never disagree.
        */}
        {sectionHighlight !== null ? (
          <div
            className="design-canvas-section-highlight"
            style={{
              left: sectionHighlight.x,
              top: stageSectionTop(sectionHighlight.y),
              width: sectionHighlight.w,
              height: sectionHighlight.h,
            }}
            aria-hidden="true"
          />
        ) : null}
        {hoveredHighlight !== null ? (
          <div
            className="design-canvas-section-highlight design-canvas-section-hover"
            style={{
              left: hoveredHighlight.x,
              top: hoveredHighlight.y,
              width: hoveredHighlight.w,
              height: hoveredHighlight.h,
            }}
            aria-hidden="true"
          />
        ) : null}
        {/*
          Direct-on-canvas selection zones: one transparent parent-side button
          per measured section, painted largest-first (see sectionOverlays) so
          the smallest — the deepest — is on top and receives the pointer.
          The display iframe keeps pointer-events:none and inert and is never
          touched: these siblings over it are what the pointer hits. Hover
          highlights, click selects through the shared onSelectLayer, so the
          canvas and the Layers panel are one state, not two.
        */}
        {sectionOverlays.map((section) => (
          <button
            key={section.id}
            type="button"
            className="design-canvas-section-overlay"
            style={{
              left: section.transform.x,
              top: section.transform.y,
              width: section.transform.width,
              height: section.transform.height,
            }}
            aria-label={`Select ${section.name}`}
            aria-pressed={selectedLayerId === section.id}
            onClick={(event) => {
              // The canvas click handler below would hit-test the same point
              // and pick the same id, but stopping here keeps one path.
              event.stopPropagation();
              onSelectLayer(section.id);
            }}
            onMouseEnter={() => setHoveredSectionId(section.id)}
            onMouseLeave={() =>
              setHoveredSectionId((current) => (current === section.id ? null : current))
            }
            onFocus={() => setHoveredSectionId(section.id)}
            onBlur={() =>
              setHoveredSectionId((current) => (current === section.id ? null : current))
            }
          />
        ))}
        {noteMarks.map((mark) => {
          const marked = sectionLayers.find((section) => section.id === mark.id);
          return (
            <span
              key={mark.id}
              className="design-canvas-note-mark"
              style={{ left: mark.x, top: stageSectionTop(mark.y) }}
              title={marked ? `Note on ${marked.name}` : "Section note"}
              aria-hidden="true"
            />
          );
        })}
      </div>
    </div>
  );
});
