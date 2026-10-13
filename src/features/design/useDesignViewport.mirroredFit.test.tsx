// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useEffect } from "react";
import { useAppStore } from "../../store/appStore";
import type { DesignHost } from "./designHost";
import type { Pan } from "../../lib/canvas/viewportMath";
import { nodesBounds } from "../../lib/canvas/viewportMath";
import { ARTIFACT_PAGE_HEIGHT } from "./artifactViewport";
import { artifactNodeRect, DESIGN_FIT_MARGIN } from "./designCanvasGeometry";
import { fitViewport, type DesignViewport } from "./designViewport";
import { useDesignViewport } from "./useDesignViewport";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const TEST_DOCUMENT = {
  name: "",
  path: "",
  contextPrefix: "Editing",
  draftPlaceholder: "Describe the change…",
  noContextPlaceholder: "Describe what to generate…",
  initialState: { zoom: 1, saved: false, draft: "", hiddenLayerIds: [] },
  selectedLayerId: "",
  grounded: true,
  layers: [],
  messages: [],
  workingMessage: { title: "Generating…", desc: "Asking the agent." },
};

const HOST: DesignHost = {
  loadDocument: async () => ({ ...TEST_DOCUMENT }),
};

const ARTIFACT_HTML = "<main>mirrored page</main>";

function setHost(): void {
  useAppStore.setState({
    activeSurface: "workspace",
    designSession: {
      host: HOST,
      document: null,
      messages: [],
      latestArtifact: null,
      generation: null,
      sectionNotes: [],
      lastCanvasPageHeight: null,
      mirroredFitPending: false,
    },
  });
  useAppStore.getState().setDesignDocument(HOST, { ...TEST_DOCUMENT }, []);
}

function clearHost(): void {
  useAppStore.setState({
    activeSurface: "workspace",
    designSession: {
      host: null,
      document: null,
      messages: [],
      latestArtifact: null,
      generation: null,
      sectionNotes: [],
      lastCanvasPageHeight: null,
      mirroredFitPending: false,
    },
  });
}

interface HarnessResult {
  pan: Pan;
  zoom: number;
  artifactPageHeight: number;
  fitCanvas: () => void;
  handleCanvasViewportChange: (viewport: DesignViewport) => void;
}

function Harness({
  surface,
  artifactHtml,
  onResult,
}: {
  surface: HTMLElement;
  artifactHtml: string | undefined;
  onResult: (result: HarnessResult) => void;
}) {
  const result = useDesignViewport({
    host: HOST,
    initialViewState: { pan: { x: 0, y: 0 }, selectedLayerId: "", zoom: 1 },
    layers: [],
    hiddenLayerIds: [],
    artifactHtml,
    artifactError: undefined,
    surfaceRef: { current: surface },
  });
  useEffect(() => {
    onResult(result);
  });
  return null;
}

describe("explicit mirrored fit", () => {
  let outer: HTMLElement;
  let canvas: HTMLDivElement;
  let hostEl: HTMLDivElement | null = null;
  let root: Root | null = null;
  let latest: HarnessResult | null = null;

  beforeEach(() => {
    setHost();
    outer = document.createElement("section");
    canvas = document.createElement("div");
    canvas.className = "design-canvas";
    outer.appendChild(canvas);
    document.body.appendChild(outer);
    Object.defineProperty(canvas, "getBoundingClientRect", {
      configurable: true,
      value: () => ({ width: 800, height: 600, left: 0, top: 0 }),
    });
    latest = null;
  });

  afterEach(async () => {
    if (root !== null) await act(async () => root?.unmount());
    root = null;
    hostEl?.remove();
    hostEl = null;
    outer.remove();
    clearHost();
  });

  async function mount(artifactHtml: string | undefined): Promise<void> {
    hostEl = document.createElement("div");
    document.body.appendChild(hostEl);
    root = createRoot(hostEl);
    await act(async () => {
      root?.render(
        <Harness
          surface={outer}
          artifactHtml={artifactHtml}
          onResult={(result) => {
            latest = result;
          }}
        />,
      );
    });
  }

  it("shares the measured canvas height with the store", async () => {
    await mount(ARTIFACT_HTML);

    expect(latest?.artifactPageHeight).toBe(ARTIFACT_PAGE_HEIGHT);
    expect(useAppStore.getState().designSession.lastCanvasPageHeight).toBe(ARTIFACT_PAGE_HEIGHT);
  });

  it("fits a preloaded artifact on an explicit open after measurement", async () => {
    await mount(ARTIFACT_HTML);
    expect(latest?.artifactPageHeight).toBe(ARTIFACT_PAGE_HEIGHT);

    await act(async () => {
      useAppStore.getState().requestMirroredFit(HOST);
    });

    const desired = 960;
    expect(latest?.artifactPageHeight).toBe(desired);
    expect(useAppStore.getState().designSession.lastCanvasPageHeight).toBe(desired);
    expect(useAppStore.getState().designSession.mirroredFitPending).toBe(false);
    const expected = fitViewport(
      nodesBounds([artifactNodeRect([], desired)]),
      800,
      600,
      DESIGN_FIT_MARGIN,
    );
    expect(latest?.zoom).toBeCloseTo(expected.zoom, 10);
    expect(latest?.pan.x).toBeCloseTo(expected.pan.x, 10);
    expect(latest?.pan.y).toBeCloseTo(expected.pan.y, 10);
  });

  it("leaves a preloaded artifact alone on an ordinary mount", async () => {
    await mount(ARTIFACT_HTML);

    expect(latest?.pan).toEqual({ x: 0, y: 0 });
    expect(latest?.zoom).toBe(1);
    expect(useAppStore.getState().designSession.mirroredFitPending).toBe(false);
  });

  it("keeps a manual camera after the explicit fit instead of refitting it", async () => {
    await mount(ARTIFACT_HTML);
    await act(async () => {
      useAppStore.getState().requestMirroredFit(HOST);
    });
    const fittedPan = latest?.pan;
    const fittedZoom = latest?.zoom;
    expect(useAppStore.getState().designSession.mirroredFitPending).toBe(false);

    await act(async () => {
      latest?.handleCanvasViewportChange({ pan: { x: 10, y: 20 }, zoom: 1.5 });
    });
    expect(latest?.pan).toEqual({ x: 10, y: 20 });
    expect(latest?.zoom).toBe(1.5);
    expect(latest?.pan).not.toEqual(fittedPan);
    expect(latest?.zoom).not.toBe(fittedZoom);
  });
});
