import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { RefObject } from "react";
import { nodesBounds } from "../../lib/canvas/viewportMath";
import type { Pan } from "../../lib/canvas/viewportMath";
import type { NodeRect } from "../../types/geometry";
import type { DesignLayer } from "./designHost";
import {
  ARTIFACT_PAGE_HEIGHT,
  artifactPageHeightForCanvas,
  shouldAdaptArtifactHeight,
} from "./artifactViewport";
import { clampViewportZoom, fitViewport, type DesignViewport } from "./designViewport";
import { DESIGN_FIT_MARGIN, artifactNodeRect, layerRectsFor } from "./designCanvasGeometry";
import type { DesignViewState } from "./designSurfaceTypes";

interface UseDesignViewportInput {
  initialViewState: DesignViewState;
  layers: readonly DesignLayer[];
  hiddenLayerIds: readonly string[];
  artifactHtml: string | undefined;
  artifactError: string | undefined;
  surfaceRef: RefObject<HTMLElement | null>;
}

interface UseDesignViewportResult {
  pan: Pan;
  selectedLayerId: string;
  zoom: number;
  artifactPageHeight: number;
  artifactRect: NodeRect | null;
  setSelectedLayerId: (layerId: string) => void;
  handleCanvasViewportChange: (nextViewport: DesignViewport) => void;
  zoomIn: () => void;
  zoomOut: () => void;
  zoomReset: () => void;
  fitCanvas: () => void;
}

export function useDesignViewport(input: UseDesignViewportInput): UseDesignViewportResult {
  // Selection shares viewState because view state lives outside history, so undo never moves the camera or the selection.
  const [viewState, setViewState] = useState<DesignViewState>(input.initialViewState);
  // Adaptive frame height for the generated page: width stays 1280, height
  // follows the live canvas aspect (see artifactViewport). Seeded at the
  // 800 baseline so mount and tests without a measured canvas keep the
  // canonical sheet until a real canvas size arrives with an artifact.
  const [artifactPageHeight, setArtifactPageHeight] = useState(ARTIFACT_PAGE_HEIGHT);

  const artifactRect = useMemo(
    () =>
      input.artifactHtml !== undefined || input.artifactError !== undefined
        ? artifactNodeRect(input.layers, artifactPageHeight)
        : null,
    [input.artifactError, input.artifactHtml, artifactPageHeight, input.layers],
  );

  const fitRects = useMemo<NodeRect[]>(() => {
    const rects = layerRectsFor(input.layers).filter(
      (layer) => !input.hiddenLayerIds.includes(layer.id),
    );
    return artifactRect === null ? rects : [...rects, artifactRect];
  }, [artifactRect, input.hiddenLayerIds, input.layers]);
  const fitRectsRef = useRef<NodeRect[]>([]);
  useEffect(() => {
    fitRectsRef.current = fitRects;
  }, [fitRects]);

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
  // True once the user pans, zooms, or wheels after the last fit, so a later
  // reframe never tears the viewport out from under their hands. fitCanvas
  // clears it; every manual viewport path sets it.
  const viewportTouchedRef = useRef(false);
  const setZoom = useCallback((nextZoom: number | ((currentZoom: number) => number)) => {
    viewportTouchedRef.current = true;
    setViewState((current) => {
      const requested = typeof nextZoom === "function" ? nextZoom(current.zoom) : nextZoom;
      const next = clampViewportZoom(requested);
      return current.zoom === next ? current : { ...current, zoom: next };
    });
  }, []);
  const handleCanvasViewportChange = useCallback(
    (nextViewport: DesignViewport) => {
      viewportTouchedRef.current = true;
      setViewport(nextViewport);
    },
    [setViewport],
  );
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
    const canvas = input.surfaceRef.current?.querySelector<HTMLElement>(".design-canvas");
    if (!canvas) return;
    const bounds = canvas.getBoundingClientRect();
    const { pan: fittedPan, zoom: fittedZoom } = fitViewport(
      nodesBounds(fitRectsRef.current),
      bounds.width,
      bounds.height,
      DESIGN_FIT_MARGIN,
    );
    viewportTouchedRef.current = false;
    setViewport({ pan: fittedPan, zoom: fittedZoom });
  }, [input.surfaceRef, setViewport]);

  // The artifact frame follows the live canvas aspect (width stays 1280, height
  // adapts), but its height re-renders the iframe, so it must not chase every
  // pixel. The ratio gate in shouldAdaptArtifactHeight and the new-artifact
  // trigger below are the only two reframe paths.
  const lastCanvasSizeRef = useRef<{ width: number; height: number } | null>(null);
  useEffect(() => {
    const canvas = input.surfaceRef.current?.querySelector<HTMLElement>(".design-canvas");
    if (!canvas || typeof ResizeObserver === "undefined") return;
    const seed = canvas.getBoundingClientRect();
    lastCanvasSizeRef.current = { width: seed.width, height: seed.height };
    const observer = new ResizeObserver(() => {
      const rect = canvas.getBoundingClientRect();
      const prev = lastCanvasSizeRef.current;
      lastCanvasSizeRef.current = { width: rect.width, height: rect.height };
      if (prev === null) return;
      if (!shouldAdaptArtifactHeight(prev.width, prev.height, rect.width, rect.height)) return;
      const desired = artifactPageHeightForCanvas(rect.width, rect.height);
      setArtifactPageHeight((current) => (current === desired ? current : desired));
    });
    observer.observe(canvas);
    return () => observer.disconnect();
  }, [input.surfaceRef]);

  // A new artifact is a full page, not a thumbnail: fit it into view the moment
  // it lands, so the whole generated page is visible without a manual Fit. The
  // ref is seeded with the artifact already on screen at mount, so reopening a
  // document keeps the saved viewport instead of snapping the camera. A reframe
  // (new artifact or adapted height) refits only while the viewport is still
  // pristine after the last fit; a manual pan/zoom owns the camera from then on.
  const fittedArtifactRef = useRef<string | undefined>(input.artifactHtml ?? input.artifactError);
  const fittedHeightRef = useRef(artifactPageHeight);
  useEffect(() => {
    const artifact = input.artifactHtml ?? input.artifactError;
    if (artifact === undefined) return;
    const isNewArtifact = fittedArtifactRef.current !== artifact;
    if (isNewArtifact) {
      fittedArtifactRef.current = artifact;
      const canvas = input.surfaceRef.current?.querySelector<HTMLElement>(".design-canvas");
      if (canvas) {
        const rect = canvas.getBoundingClientRect();
        lastCanvasSizeRef.current = { width: rect.width, height: rect.height };
        const desired = artifactPageHeightForCanvas(rect.width, rect.height);
        if (desired !== artifactPageHeight) {
          // Defer the fit until the reframed height commits, so the camera
          // fits the sheet the user will actually see instead of the old one.
          setArtifactPageHeight(desired);
          return;
        }
      }
    }
    const heightChanged = fittedHeightRef.current !== artifactPageHeight;
    if (!isNewArtifact && !heightChanged) return;
    fittedHeightRef.current = artifactPageHeight;
    if (!viewportTouchedRef.current) fitCanvas();
  }, [input.artifactError, input.artifactHtml, artifactPageHeight, fitCanvas, input.surfaceRef]);

  const setSelectedLayerId = useCallback((layerId: string) => {
    setViewState((current) =>
      current.selectedLayerId === layerId ? current : { ...current, selectedLayerId: layerId },
    );
  }, []);

  const { pan, selectedLayerId, zoom } = viewState;

  return {
    pan,
    selectedLayerId,
    zoom,
    artifactPageHeight,
    artifactRect,
    setSelectedLayerId,
    handleCanvasViewportChange,
    zoomIn,
    zoomOut,
    zoomReset,
    fitCanvas,
  };
}
