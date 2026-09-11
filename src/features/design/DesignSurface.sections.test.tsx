// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import {
  artifactNodeRect,
  buildLayerTree,
  DesignSurface,
  layerAncestorIds,
  layerMoveTarget,
  revealScrollTopFor,
  smallestSectionAt,
  type DesignDocument,
  type DesignHost,
} from "./DesignSurface";
import {
  clearCachedArtifactSections,
  setCachedArtifactSections,
  type ArtifactSection,
} from "./artifactStructure";
import type { DesignLayer } from "./designHost";

const settingsMocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  loadProvider: vi.fn(),
  loadStoredProvider: vi.fn(),
  saveProvider: vi.fn(),
  loadWorkspace: vi.fn(),
  loadStoredWorkspace: vi.fn(),
  saveWorkspace: vi.fn(),
  loadOutput: vi.fn(),
  saveOutput: vi.fn(),
}));

const providerMocks = vi.hoisted(() => ({
  daemonStatus: vi.fn(),
  list: vi.fn(),
  projectsList: vi.fn(),
  workspacesList: vi.fn(),
}));

vi.mock("./designSettings", async () => {
  const actual = await vi.importActual<typeof import("./designSettings")>("./designSettings");
  return {
    ...actual,
    loadDesignSkillSelection: settingsMocks.load,
    saveDesignSkillSelection: settingsMocks.save,
    loadDesignProviderId: settingsMocks.loadProvider,
    loadStoredDesignProviderId: settingsMocks.loadStoredProvider,
    saveDesignProviderId: settingsMocks.saveProvider,
    loadDesignWorkspaceId: settingsMocks.loadWorkspace,
    loadStoredDesignWorkspaceId: settingsMocks.loadStoredWorkspace,
    saveDesignWorkspaceId: settingsMocks.saveWorkspace,
    loadDesignOutputMode: settingsMocks.loadOutput,
    saveDesignOutputMode: settingsMocks.saveOutput,
  };
});

vi.mock("./DesignHistoryList", () => ({
  DesignHistoryList: () => null,
}));

vi.mock("./designHistory", () => ({
  recordDesignHistoryEntry: vi.fn(async () => true),
}));

vi.mock("./designHistoryOpen", () => ({
  openDesignHistoryEntry: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  daemonStatus: providerMocks.daemonStatus,
  providersList: providerMocks.list,
  projectsList: providerMocks.projectsList,
  workspacesList: providerMocks.workspacesList,
  reasonFromCause: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
  createSessionStateChannel: vi.fn(),
  sessionCreate: vi.fn(),
  sessionsList: vi.fn(),
  sessionsUnwatch: vi.fn(),
  sessionsWatch: vi.fn(),
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const DOCUMENT: DesignDocument = {
  name: "",
  path: "",
  contextPrefix: "Editing",
  draftPlaceholder: "Describe the change…",
  noContextPlaceholder: "Describe what to generate…",
  selectedLayerId: "",
  grounded: true,
  initialState: {
    zoom: 1,
    saved: true,
    draft: "",
    hiddenLayerIds: [],
  },
  layers: [],
  messages: [],
  workingMessage: {
    title: "Generating…",
    desc: "Asking the agent.",
  },
};

const ARTIFACT_HTML =
  "<header><h1>Shop</h1></header><main><section><h2>Deals</h2></section></main>";

// The measured list the collector posts: two landmarks at the root and the
// headings below them. `parent` is an index in this list, exactly as the frame
// emits it; the surface turns it into a layer id.
const ARTIFACT_SECTIONS = [
  {
    anchor: "body[1]/header[1]",
    tag: "header",
    name: "Shop",
    depth: 1,
    parent: null,
    rect: { x: 0, y: 0, width: 1280, height: 120 },
  },
  {
    anchor: "body[1]/header[1]/h1[1]",
    tag: "h1",
    name: "Shop",
    depth: 2,
    parent: 0,
    rect: { x: 40, y: 20, width: 400, height: 60 },
  },
  {
    anchor: "body[1]/main[1]",
    tag: "main",
    name: "Deals",
    depth: 1,
    parent: null,
    rect: { x: 0, y: 120, width: 1280, height: 600 },
  },
  {
    anchor: "body[1]/main[1]/section[1]",
    tag: "section",
    name: "Deals",
    depth: 2,
    parent: 2,
    rect: { x: 40, y: 160, width: 1200, height: 400 },
  },
  {
    anchor: "body[1]/main[1]/section[1]/h2[1]",
    tag: "h2",
    name: "Deals",
    depth: 3,
    parent: 3,
    rect: { x: 60, y: 180, width: 600, height: 40 },
  },
] as const;

// A page whose root level is two slides and whose leaves are named distinctly,
// so a test can select one phrase and walk the chain back to its slide.
const TREE_HTML =
  '<section id="slide-1"><h2 id="s1-title">One</h2><p id="s1-body">Body</p></section>' +
  '<section id="slide-2"><h2 id="s2-title">Two</h2></section>';
const TREE_SECTIONS = [
  {
    anchor: "slide-1",
    tag: "section",
    name: "Slide one",
    depth: 1,
    parent: null,
    rect: { x: 0, y: 0, width: 1280, height: 400 },
  },
  {
    anchor: "s1-title",
    tag: "h2",
    name: "Title one",
    depth: 2,
    parent: 0,
    rect: { x: 40, y: 20, width: 600, height: 60 },
  },
  {
    anchor: "s1-body",
    tag: "p",
    name: "Body copy",
    depth: 2,
    parent: 0,
    rect: { x: 40, y: 100, width: 600, height: 40 },
  },
  {
    anchor: "slide-2",
    tag: "section",
    name: "Slide two",
    depth: 1,
    parent: null,
    rect: { x: 0, y: 400, width: 1280, height: 400 },
  },
  {
    anchor: "s2-title",
    tag: "h2",
    name: "Title two",
    depth: 2,
    parent: 3,
    rect: { x: 40, y: 420, width: 600, height: 60 },
  },
] as const;

const GENERATION_BASE = {
  sessionId: "session-sections",
  peerSessionId: null,
  createdAtMs: 1_000,
  prompt: "host echo",
  title: "Generated result",
  desc: "Generated by the host.",
  sources: [],
  nodeIds: [],
};

// Scroll fixtures: a page taller than the frame window, with one section in the
// first screen and one well below it.
const SCROLL_HTML = "<section><h2>Top</h2></section><section><h2>Far</h2></section>";
const SCROLL_SECTIONS = [
  {
    anchor: "body[1]/section[1]",
    tag: "section",
    name: "Top",
    depth: 1,
    rect: { x: 0, y: 0, width: 1280, height: 200 },
  },
  {
    anchor: "body[1]/section[2]",
    tag: "section",
    name: "Far",
    depth: 1,
    rect: { x: 0, y: 1400, width: 1280, height: 400 },
  },
] as const;
const SCROLL_CONTENT_HEIGHT = 3600;
const SCROLL_ARTIFACT_ORIGIN = { x: 60, y: 46 };

function createHost(overrides: Partial<DesignHost> = {}): DesignHost {
  return {
    loadDocument: vi.fn(async () => DOCUMENT),
    saveDocument: vi.fn(async () => undefined),
    ...overrides,
  };
}

async function renderDesign(host: DesignHost): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<DesignSurface host={host} />);
    await Promise.resolve();
    await Promise.resolve();
  });
  return { container, root };
}

async function fillDraft(container: HTMLDivElement, prompt: string): Promise<void> {
  const draft = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Describe a design change"]',
  );
  if (draft === null) throw new Error("Design composer missing");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("textarea value setter did not exist");
  await act(async () => {
    setValue.call(draft, prompt);
    draft.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

async function generateArtifact(
  container: HTMLDivElement,
  generate: ReturnType<typeof vi.fn>,
  prompt: string,
  contentHeight?: number,
): Promise<void> {
  setCachedArtifactSections(ARTIFACT_HTML, [...ARTIFACT_SECTIONS], contentHeight);
  await fillDraft(container, prompt);
  const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
  if (send === null) throw new Error("Generate control missing");
  await act(async () => {
    send.click();
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(generate).toHaveBeenCalled();
}

function layerRowByName(container: HTMLDivElement, name: string): HTMLButtonElement {
  const rows = Array.from(container.querySelectorAll<HTMLButtonElement>(".design-layer-select"));
  const row = rows.find((candidate) => candidate.textContent?.includes(name));
  if (row === undefined) throw new Error(`Layer row for ${name} missing`);
  return row;
}

function stageViewport(container: HTMLDivElement): {
  panX: number;
  panY: number;
  zoom: number;
} {
  const stage = container.querySelector<HTMLElement>(".design-canvas-stage");
  const transform = stage?.style.transform ?? "";
  const match = /translate\((-?[\d.]+)px, (-?[\d.]+)px\) scale\(([\d.]+)\)/.exec(transform);
  if (match === null) throw new Error(`Stage transform missing: ${transform}`);
  return { panX: Number(match[1]), panY: Number(match[2]), zoom: Number(match[3]) };
}

function clientPointForWorld(
  container: HTMLDivElement,
  x: number,
  y: number,
): { clientX: number; clientY: number } {
  const { panX, panY, zoom } = stageViewport(container);
  return { clientX: x * zoom + panX, clientY: y * zoom + panY };
}

function wheelEvent(init: {
  clientX: number;
  clientY: number;
  deltaY: number;
  deltaMode?: number;
  shiftKey?: boolean;
}): WheelEvent {
  const event = new Event("wheel", { bubbles: true, cancelable: true }) as WheelEvent;
  Object.defineProperties(event, {
    clientX: { value: init.clientX },
    clientY: { value: init.clientY },
    deltaMode: { value: init.deltaMode ?? 0 },
    deltaY: { value: init.deltaY },
    shiftKey: { value: init.shiftKey ?? false },
  });
  return event;
}

async function fillNote(container: HTMLDivElement, text: string): Promise<void> {
  const field = container.querySelector<HTMLInputElement>(
    'input[aria-label="Note for the agent on this section"]',
  );
  if (field === null) throw new Error("Note field missing");
  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("input value setter did not exist");
  await act(async () => {
    setValue.call(field, text);
    field.dispatchEvent(new Event("input", { bubbles: true }));
  });
  const add = Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
    (button) => button.textContent === "Add",
  );
  if (add === undefined) throw new Error("Add-note control missing");
  await act(async () => {
    add.click();
    await Promise.resolve();
  });
}

async function generateCachedArtifact(
  container: HTMLDivElement,
  generate: ReturnType<typeof vi.fn>,
  html: string,
  sections: readonly ArtifactSection[],
  prompt: string,
): Promise<void> {
  setCachedArtifactSections(html, [...sections]);
  await fillDraft(container, prompt);
  const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
  if (send === null) throw new Error("Generate control missing");
  await act(async () => {
    send.click();
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(generate).toHaveBeenCalled();
}

async function selectOverlay(container: HTMLDivElement, name: string): Promise<void> {
  const overlays = Array.from(
    container.querySelectorAll<HTMLButtonElement>(".design-canvas-section-overlay"),
  );
  const overlay = overlays.find(
    (candidate) => candidate.getAttribute("aria-label") === `Select ${name}`,
  );
  if (overlay === undefined) throw new Error(`Section overlay for ${name} missing`);
  await act(async () => {
    overlay.click();
    await Promise.resolve();
  });
}

function selectedAnchor(container: HTMLDivElement): string | null {
  return container.querySelector(".design-layer-details .design-layer-anchor")?.textContent ?? null;
}

async function pressArrow(container: HTMLDivElement, key: string): Promise<void> {
  const surface = container.querySelector<HTMLElement>(".design-surface");
  if (surface === null) throw new Error("Design surface missing");
  await act(async () => {
    surface.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
    await Promise.resolve();
  });
}

beforeEach(() => {
  useAppStore.setState({ plugins: null, installing: null, installError: null });
  settingsMocks.load.mockResolvedValue({ version: 1, mode: "all", enabledSlugs: [] });
  settingsMocks.save.mockResolvedValue(true);
  settingsMocks.loadProvider.mockResolvedValue(null);
  settingsMocks.loadStoredProvider.mockResolvedValue(null);
  settingsMocks.saveProvider.mockResolvedValue(true);
  settingsMocks.loadWorkspace.mockResolvedValue(null);
  settingsMocks.loadOutput.mockResolvedValue("page");
  settingsMocks.saveOutput.mockResolvedValue(true);
  settingsMocks.loadStoredWorkspace.mockResolvedValue(null);
  settingsMocks.saveWorkspace.mockResolvedValue(true);
  providerMocks.list.mockResolvedValue({ providers: [], unreadableDirs: 0 });
  providerMocks.daemonStatus.mockResolvedValue({ capabilities: [] });
  providerMocks.projectsList.mockResolvedValue([]);
  providerMocks.workspacesList.mockResolvedValue([]);
});

afterEach(() => {
  document.body.replaceChildren();
  clearCachedArtifactSections();
});

describe("artifactNodeRect with section layers", () => {
  it("ignores section layers so the frame never depends on its own sections", () => {
    const canvasLayer = {
      id: "panel",
      name: "Panel",
      kind: "TSX" as const,
      transform: { x: 60, y: 46, width: 300, height: 124 },
    };
    const sectionLayer = {
      id: "section:body[1]/main[1]",
      name: "Deals",
      kind: "SECTION" as const,
      transform: { x: 60, y: 166, width: 1280, height: 600 },
      section: { tag: "main", anchor: "body[1]/main[1]" },
    };
    expect(artifactNodeRect([canvasLayer, sectionLayer], 800)).toEqual(
      artifactNodeRect([canvasLayer], 800),
    );
    expect(artifactNodeRect([], 800)).toMatchObject({ x: 60, y: 46, w: 1280, h: 800 });
  });
});

describe("smallestSectionAt", () => {
  const outer = {
    id: "section:outer",
    name: "Outer",
    kind: "SECTION" as const,
    transform: { x: 0, y: 0, width: 100, height: 100 },
  };
  const inner = {
    id: "section:inner",
    name: "Inner",
    kind: "SECTION" as const,
    transform: { x: 10, y: 10, width: 20, height: 20 },
  };

  it("picks the smallest containing rect when sections nest", () => {
    // Nested or not, document order must not matter: the deepest wins.
    expect(smallestSectionAt([outer, inner], [], { x: 15, y: 15 })?.id).toBe("section:inner");
    expect(smallestSectionAt([inner, outer], [], { x: 15, y: 15 })?.id).toBe("section:inner");
    expect(smallestSectionAt([outer, inner], [], { x: 50, y: 50 })?.id).toBe("section:outer");
  });

  it("returns null outside every section and skips hidden ones", () => {
    expect(smallestSectionAt([outer, inner], [], { x: 500, y: 500 })).toBeNull();
    expect(smallestSectionAt([outer, inner], ["section:inner"], { x: 15, y: 15 })?.id).toBe(
      "section:outer",
    );
  });
});

describe("DesignSurface direct-on-canvas section selection", () => {
  it("paints one transparent overlay per section, smallest on top", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    const overlays = container.querySelectorAll<HTMLButtonElement>(
      ".design-canvas-section-overlay",
    );
    expect(overlays).toHaveLength(5);
    // Largest-first paint order: the smallest (deepest) overlay is on top and
    // receives the pointer, matching smallestSectionAt.
    const areas = Array.from(overlays).map((overlay) => {
      const width = Number.parseFloat(overlay.style.width);
      const height = Number.parseFloat(overlay.style.height);
      return width * height;
    });
    expect(areas).toEqual([...areas].sort((left, right) => right - left));
    // The display iframe itself stays out of the pointer path.
    const frame = container.querySelector<HTMLElement>(".design-artifact-frame");
    expect(frame?.style.pointerEvents).toBe("none");
    await act(async () => root.unmount());
  });

  it("selects from the canvas into the same state as the panel", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    // Last overlay is the smallest: the h2 (600 × 40) inside the section.
    const overlays = container.querySelectorAll<HTMLButtonElement>(
      ".design-canvas-section-overlay",
    );
    const target = overlays[overlays.length - 1];
    if (target === undefined) throw new Error("Section overlays missing");
    await act(async () => {
      target.click();
      await Promise.resolve();
    });

    const details = container.querySelector(".design-layer-details");
    if (details === null) throw new Error("Section details missing after overlay click");
    expect(details.textContent).toContain("body[1]/main[1]/section[1]/h2[1]");
    // One state, not two: the panel row for the same section marks selected.
    const selectedRows = container.querySelectorAll(".design-layer-row-selected");
    expect(selectedRows).toHaveLength(1);
    const highlight = container.querySelector<HTMLElement>(".design-canvas-section-highlight");
    if (highlight === null) throw new Error("Section highlight missing");
    expect(highlight.style.width).toBe("600px");
    expect(highlight.style.height).toBe("40px");
    await act(async () => root.unmount());
  });

  it("highlights on hover without selecting", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    const overlays = container.querySelectorAll<HTMLButtonElement>(
      ".design-canvas-section-overlay",
    );
    const target = overlays[overlays.length - 1];
    if (target === undefined) throw new Error("Section overlays missing");
    await act(async () => {
      target.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
      await Promise.resolve();
    });

    const hover = container.querySelector<HTMLElement>(".design-canvas-section-hover");
    if (hover === null) throw new Error("Section hover highlight missing");
    expect(hover.style.width).toBe("600px");
    // Hover alone selects nothing: no expanded row, no marked row.
    expect(container.querySelector(".design-layer-details")).toBeNull();
    expect(container.querySelector(".design-layer-row-selected")).toBeNull();

    await act(async () => {
      target.dispatchEvent(new MouseEvent("mouseout", { bubbles: true }));
      await Promise.resolve();
    });
    expect(container.querySelector(".design-canvas-section-hover")).toBeNull();
    await act(async () => root.unmount());
  });
});

describe("DesignSurface page sections", () => {
  it("lists only the root sections in the short navigator, with their count", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    expect(container.querySelector(".design-layers-panel")).toBeNull();

    await generateArtifact(container, generate, "Build a shop page.");

    const panel = container.querySelector(".design-layers-panel");
    if (panel === null) throw new Error("Layers panel missing after measurement");
    const kinds = Array.from(panel.querySelectorAll(".design-layer-kind")).map(
      (badge) => badge.textContent,
    );
    // Only the landmarks at the root: the h1 under header and the section and
    // h2 under main are discovered by clicking, not listed up front.
    expect(kinds).toEqual(["header", "main"]);
    expect(panel.querySelector(".design-layer-count")?.textContent).toBe("2");
    expect(panel.querySelectorAll(".design-layer-select")).toHaveLength(2);
    expect(panel.textContent).toContain("Deals");
    // Every measured section still has its own canvas hit zone.
    expect(container.querySelectorAll(".design-canvas-section-overlay")).toHaveLength(5);
    await act(async () => root.unmount());
  });

  it("expands the selected section row in place with a canvas highlight", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    await act(async () => {
      layerRowByName(container, "Deals").click();
      await Promise.resolve();
    });

    // One panel only: the row expands where it is, and the canvas keeps its
    // full width instead of reserving a second panel's share.
    expect(container.querySelector(".design-inspector-panel")).toBeNull();
    expect(container.querySelector(".design-workspace-inspector-open")).toBeNull();
    const selectedRow = container.querySelector(".design-layer-row-selected");
    if (selectedRow === null) throw new Error("Selected section row missing");
    // The tag stays the row badge; the expanded row adds only what the row
    // does not already say: the anchor and the measured size.
    expect(selectedRow.querySelector(".design-layer-kind")?.textContent).toBe("main");
    const details = selectedRow.querySelector(".design-layer-details");
    if (details === null) throw new Error("Section details missing");
    expect(details.textContent).toContain("body[1]/main[1]");
    expect(details.textContent).toContain("1280 × 600 px");
    expect(
      details.querySelector('input[aria-label="Note for the agent on this section"]'),
    ).not.toBeNull();
    // Canvas-node controls are meaningless for a measured section.
    expect(details.querySelector(".design-radius-option")).toBeNull();
    expect(details.textContent).not.toContain("Corners");
    expect(details.textContent).not.toContain("Elevation");
    expect(details.textContent).not.toContain("Duplicate");
    expect(details.textContent).not.toContain("Delete");

    // World highlight: artifact origin (60, 46) plus the measured page rect.
    const highlight = container.querySelector<HTMLElement>(".design-canvas-section-highlight");
    if (highlight === null) throw new Error("Section highlight missing");
    expect(highlight.style.left).toBe("60px");
    expect(highlight.style.top).toBe("166px");
    expect(highlight.style.width).toBe("1280px");
    expect(highlight.style.height).toBe("600px");
    await act(async () => root.unmount());
  });

  it("selects the section under a canvas click instead of the whole artifact", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    const canvas = container.querySelector(".design-canvas");
    if (canvas === null) throw new Error("Canvas missing");
    // World (500, 180): inside main (y 166–766) but above its section (y 206+).
    // The fitted viewport (zoom/pan) maps world to client; invert it here so
    // the click lands on the section whatever the fit produced.
    const stage = container.querySelector<HTMLElement>(".design-canvas-stage");
    const transform = stage?.style.transform ?? "";
    const match = /translate\((-?[\d.]+)px, (-?[\d.]+)px\) scale\(([\d.]+)\)/.exec(transform);
    if (match === null) throw new Error(`Stage transform missing: ${transform}`);
    const [, panX, panY, zoom] = match.map(Number);
    const clientX = 500 * (zoom ?? 1) + (panX ?? 0);
    const clientY = 180 * (zoom ?? 1) + (panY ?? 0);
    await act(async () => {
      canvas.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX, clientY }));
      await Promise.resolve();
    });

    const details = container.querySelector(".design-layer-details");
    if (details === null) throw new Error("Section details missing after canvas click");
    expect(details.textContent).toContain("body[1]/main[1]");
    await act(async () => root.unmount());
  });

  it("deselects the section from its expanded row and restores the full canvas", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    await act(async () => {
      layerRowByName(container, "Deals").click();
      await Promise.resolve();
    });
    expect(container.querySelector(".design-layer-details")).not.toBeNull();

    const deselect = container.querySelector<HTMLButtonElement>('[aria-label="Deselect section"]');
    if (deselect === null) throw new Error("Deselect control missing");
    await act(async () => {
      deselect.click();
      await Promise.resolve();
    });

    expect(container.querySelector(".design-layer-row-selected")).toBeNull();
    expect(container.querySelector(".design-layer-details")).toBeNull();
    expect(container.querySelector(".design-workspace-inspector-open")).toBeNull();
    expect(document.activeElement).toBe(container.querySelector(".design-canvas"));
    await act(async () => root.unmount());
  });

  it("shows the rounded measured size first, with the anchor below it", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    // Own flow instead of generateArtifact: that helper caches the standard
    // sections, which would overwrite this fractional-measure cache.
    setCachedArtifactSections(ARTIFACT_HTML, [
      {
        anchor: "body[1]/div[1]/div[1]/div[1]/nav[1]",
        tag: "nav",
        name: "Nav",
        depth: 4,
        rect: { x: 10, y: 10, width: 286.84, height: 21.7 },
      },
    ]);
    await fillDraft(container, "Build a nav.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => {
      send.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(generate).toHaveBeenCalled();

    // One root is no navigator: nothing to jump between, so the panel waits for
    // the layer to be discovered by clicking. This is also the only panel path
    // for a single-`main` page.
    expect(container.querySelector(".design-layers-panel")).toBeNull();
    const overlay = container.querySelector<HTMLButtonElement>(".design-canvas-section-overlay");
    if (overlay === null) throw new Error("Section overlay missing");
    await act(async () => {
      overlay.click();
      await Promise.resolve();
    });

    const details = container.querySelector(".design-layer-details");
    if (details === null) throw new Error("Section details missing");
    // The chain holds only the layer itself, so no breadcrumb is drawn.
    expect(container.querySelector(".design-layer-trail")).toBeNull();
    const measured = details.querySelector(".design-layer-measured");
    const anchor = details.querySelector(".design-layer-anchor");
    if (measured === null || anchor === null) throw new Error("Diagnostics lines missing");
    // Rounded to integers: the fractional measure never reaches the user.
    expect(measured.textContent).toBe("287 × 22 px");
    expect(details.textContent).not.toContain("286.84");
    expect(details.textContent).not.toContain("21.7");
    expect(anchor.textContent).toBe("body[1]/div[1]/div[1]/div[1]/nav[1]");
    // The size owns the first line; the anchor follows it.
    expect(
      measured.compareDocumentPosition(anchor) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    await act(async () => root.unmount());
  });

  it("lets the note field own the first Escape instead of deselecting", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    await act(async () => {
      layerRowByName(container, "Deals").click();
      await Promise.resolve();
    });
    const field = container.querySelector<HTMLInputElement>(
      'input[aria-label="Note for the agent on this section"]',
    );
    if (field === null) throw new Error("Note field missing");
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("input value setter did not exist");
    await act(async () => {
      setValue.call(field, "half-written note");
      field.dispatchEvent(new Event("input", { bubbles: true }));
    });

    // First Escape with a draft clears the draft and keeps the selection.
    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await Promise.resolve();
    });
    expect(field.value).toBe("");
    expect(container.querySelector(".design-layer-row-selected")).not.toBeNull();
    expect(container.querySelector(".design-layer-details")).not.toBeNull();

    // Second Escape on the empty field deselects, as before.
    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await Promise.resolve();
    });
    expect(container.querySelector(".design-layer-row-selected")).toBeNull();
    expect(container.querySelector(".design-layer-details")).toBeNull();
    await act(async () => root.unmount());
  });

  it("submits the note to the current anchor after a parent re-render", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    await act(async () => {
      layerRowByName(container, "Deals").click();
      await Promise.resolve();
    });
    // Typing in the composer re-renders the surface without touching the
    // selection: the stabilized submit callback must still close over the
    // current anchor, not a stale one.
    await fillDraft(container, "Refine the deals.");
    await fillNote(container, "Make the CTA louder.");

    const dealsRow = layerRowByName(container, "Deals").closest(".design-layer-row");
    expect(dealsRow?.querySelector(".design-layer-note-dot")).not.toBeNull();
    await act(async () => root.unmount());
  });

  it("sends a section note to the agent in the scope block", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    await act(async () => {
      layerRowByName(container, "Deals").click();
      await Promise.resolve();
    });
    await fillNote(container, "Make the CTA louder.");

    // Marked on the canvas without selecting, dotted in the panel.
    expect(container.querySelector(".design-canvas-note-mark")).not.toBeNull();
    const mainRow = layerRowByName(container, "Deals").closest(".design-layer-row");
    expect(mainRow?.querySelector(".design-layer-note-dot")).not.toBeNull();

    await fillDraft(container, "Refine the deals.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => {
      send.click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(generate).toHaveBeenLastCalledWith(
      expect.stringContaining("Section notes:\n- [body[1]/main[1]]: Make the CTA louder."),
      expect.any(AbortSignal),
      expect.anything(),
    );
    await act(async () => root.unmount());
  });

  it("shows orphaned notes instead of losing them, flagged in the next prompt", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    await act(async () => {
      useAppStore
        .getState()
        .setSectionNotes(host, [{ anchor: "body[1]/main[1]/section[9]", text: "Keep me" }]);
      await Promise.resolve();
    });

    const orphanBadge = container.querySelector(".design-layer-orphan-badge");
    if (orphanBadge === null) throw new Error("Orphan badge missing");
    expect(orphanBadge.textContent).toBe("orphan");
    expect(container.querySelector(".design-layer-orphans")?.textContent).toContain(
      "body[1]/main[1]/section[9]",
    );

    await act(async () => {
      layerRowByName(container, "Deals").click();
      await Promise.resolve();
    });
    await fillDraft(container, "Refine again.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => {
      send.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(generate).toHaveBeenLastCalledWith(
      expect.stringContaining(
        "[body[1]/main[1]/section[9]] (anchor not found in the current page): Keep me",
      ),
      expect.any(AbortSignal),
      expect.anything(),
    );
    await act(async () => root.unmount());
  });

  it("saves section notes with the document", async () => {
    const saveDocument = vi.fn(async () => undefined);
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate, saveDocument });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    await act(async () => {
      useAppStore
        .getState()
        .setSectionNotes(host, [{ anchor: "body[1]/main[1]", text: "Persist me" }]);
      await Promise.resolve();
    });

    const save = container.querySelector<HTMLButtonElement>(".design-save-primary");
    if (save === null) throw new Error("Save control missing");
    await act(async () => {
      save.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(saveDocument).toHaveBeenCalledWith(
      expect.objectContaining({
        sectionNotes: [{ anchor: "body[1]/main[1]", text: "Persist me" }],
      }),
    );
    await act(async () => root.unmount());
  });
});

describe("layer tree helpers", () => {
  const transform = { x: 0, y: 0, width: 100, height: 40 };
  const layers: DesignLayer[] = [
    {
      id: "slide-1",
      name: "Slide one",
      kind: "SECTION",
      transform,
      section: { tag: "section", anchor: "slide-1" },
    },
    {
      id: "a",
      name: "A",
      kind: "SECTION",
      transform,
      section: { tag: "h2", anchor: "a", parentId: "slide-1" },
    },
    {
      id: "b",
      name: "B",
      kind: "SECTION",
      transform,
      section: { tag: "p", anchor: "b", parentId: "slide-1" },
    },
    { id: "canvas", name: "Node", kind: "TSX", transform },
  ];

  it("treats canvas nodes and parentless sections as roots", () => {
    // Canvas layers carry no section at all, so they sit at the same level as
    // the page roots instead of being stranded outside the navigator.
    expect(buildLayerTree(layers).roots.map((layer) => layer.id)).toEqual(["slide-1", "canvas"]);
  });

  it("derives the four moves from parentId alone", () => {
    const tree = buildLayerTree(layers);
    expect(layerMoveTarget(tree, "b", "parent")).toBe("slide-1");
    expect(layerMoveTarget(tree, "slide-1", "first-child")).toBe("a");
    expect(layerMoveTarget(tree, "a", "next-sibling")).toBe("b");
    expect(layerMoveTarget(tree, "b", "previous-sibling")).toBe("a");
    expect(layerMoveTarget(tree, "slide-1", "previous-sibling")).toBeNull();
    expect(layerMoveTarget(tree, "canvas", "next-sibling")).toBeNull();
    expect(layerMoveTarget(tree, "slide-1", "parent")).toBeNull();
    expect(layerMoveTarget(tree, "a", "first-child")).toBeNull();
  });

  it("rebuilds the root-to-leaf chain from ids", () => {
    const tree = buildLayerTree(layers);
    expect(layerAncestorIds(tree, "b")).toEqual(["slide-1", "b"]);
    expect(layerAncestorIds(tree, "missing")).toEqual([]);
  });
});

describe("DesignSurface layer tree navigation", () => {
  async function openTree(): Promise<{
    container: HTMLDivElement;
    root: ReturnType<typeof createRoot>;
  }> {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: TREE_HTML }));
    const rendered = await renderDesign(createHost({ generate }));
    await generateCachedArtifact(
      rendered.container,
      generate,
      TREE_HTML,
      [...TREE_SECTIONS],
      "Build two slides.",
    );
    return rendered;
  }

  it("shows the clicked phrase with its chain and climbs back to the slide", async () => {
    const { container, root } = await openTree();
    // The navigator holds the two slides, never the leaves.
    expect(
      Array.from(container.querySelectorAll(".design-layer-select")).map(
        (row) => row.querySelector(".design-layer-name")?.textContent,
      ),
    ).toEqual(["Slide one", "Slide two"]);

    await selectOverlay(container, "Body copy");
    expect(selectedAnchor(container)).toBe("s1-body");
    expect(
      Array.from(container.querySelectorAll(".design-layer-trail-step")).map(
        (step) => step.textContent,
      ),
    ).toEqual(["Slide one", "Body copy"]);
    // One expanded row only: the deep layer is the inspector, not a third
    // navigator row, so the list cannot grow with the page.
    expect(container.querySelectorAll(".design-layer-row-selected")).toHaveLength(1);

    await act(async () => {
      (container.querySelector(".design-layer-trail-step") as HTMLButtonElement).click();
      await Promise.resolve();
    });
    expect(selectedAnchor(container)).toBe("slide-1");
    // slide-1 is a root: it expands in place in the navigator, so there are
    // still exactly two select rows and no duplicate inspector row.
    expect(container.querySelectorAll(".design-layer-select")).toHaveLength(2);
    expect(container.querySelectorAll(".design-layer-row-selected")).toHaveLength(1);
    await act(async () => root.unmount());
  });

  it("moves parent, first child, and siblings with the arrow keys", async () => {
    const { container, root } = await openTree();
    await selectOverlay(container, "Body copy");
    expect(selectedAnchor(container)).toBe("s1-body");

    await pressArrow(container, "ArrowUp");
    expect(selectedAnchor(container)).toBe("slide-1");
    await pressArrow(container, "ArrowDown");
    expect(selectedAnchor(container)).toBe("s1-title");
    await pressArrow(container, "ArrowRight");
    expect(selectedAnchor(container)).toBe("s1-body");
    await pressArrow(container, "ArrowLeft");
    expect(selectedAnchor(container)).toBe("s1-title");
    await pressArrow(container, "ArrowUp");
    expect(selectedAnchor(container)).toBe("slide-1");
    await pressArrow(container, "ArrowRight");
    expect(selectedAnchor(container)).toBe("slide-2");
    await pressArrow(container, "ArrowLeft");
    expect(selectedAnchor(container)).toBe("slide-1");
    // At the root with no sibling in that direction the key keeps its default
    // instead of dead-ending: the selection simply does not move.
    await pressArrow(container, "ArrowLeft");
    expect(selectedAnchor(container)).toBe("slide-1");
    await act(async () => root.unmount());
  });

  it("leaves the arrow keys to the note field's caret", async () => {
    const { container, root } = await openTree();
    await selectOverlay(container, "Body copy");
    const field = container.querySelector<HTMLInputElement>(
      'input[aria-label="Note for the agent on this section"]',
    );
    if (field === null) throw new Error("Note field missing");
    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowUp", bubbles: true }));
      await Promise.resolve();
    });
    expect(selectedAnchor(container)).toBe("s1-body");
    await act(async () => root.unmount());
  });
});

function artifactContent(container: HTMLDivElement): HTMLElement {
  const content = container.querySelector<HTMLElement>(".design-canvas-artifact-content");
  if (content === null) throw new Error("Artifact content missing");
  return content;
}

async function generateScrollArtifact(generate: DesignHost["generate"]): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const { container, root } = await renderDesign(createHost({ generate }));
  setCachedArtifactSections(SCROLL_HTML, [...SCROLL_SECTIONS], SCROLL_CONTENT_HEIGHT);
  await fillDraft(container, "Build a tall page.");
  const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
  if (send === null) throw new Error("Generate control missing");
  await act(async () => {
    send.click();
    await Promise.resolve();
    await Promise.resolve();
  });
  expect(generate).toHaveBeenCalled();
  return { container, root };
}

async function wheelOverArtifact(
  container: HTMLDivElement,
  deltaY: number,
  shiftKey = true,
): Promise<void> {
  const canvas = container.querySelector<HTMLDivElement>(".design-canvas");
  if (canvas === null) throw new Error("Canvas missing");
  const point = clientPointForWorld(
    container,
    SCROLL_ARTIFACT_ORIGIN.x + 640,
    SCROLL_ARTIFACT_ORIGIN.y + 400,
  );
  await act(async () => {
    canvas.dispatchEvent(wheelEvent({ ...point, deltaY, shiftKey }));
    await Promise.resolve();
  });
}

describe("DesignSurface artifact window scrolling", () => {
  it("gives the frame the measured page height and starts at the top", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: SCROLL_HTML }));
    const { container, root } = await generateScrollArtifact(generate);

    const content = artifactContent(container);
    // The window stays artifactPageHeight; the frame inside it is page-sized.
    expect(content.style.height).toBe("3600px");
    expect(content.style.transform).toBe("translateY(0px)");
    const frame = container.querySelector<HTMLIFrameElement>(".design-artifact-frame");
    expect(frame?.parentElement).toBe(content);
    await act(async () => root.unmount());
  });

  it("scrolls with Shift+wheel and keeps the section hit zones on the moved content", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: SCROLL_HTML }));
    const { container, root } = await generateScrollArtifact(generate);
    const content = artifactContent(container);

    const overlaysBefore = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".design-canvas-section-overlay"),
    );
    const topsBefore = overlaysBefore.map((overlay) => Number.parseFloat(overlay.style.top));

    await wheelOverArtifact(container, 120);

    expect(content.style.transform).toBe("translateY(-120px)");
    const topsAfter = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".design-canvas-section-overlay"),
    ).map((overlay) => Number.parseFloat(overlay.style.top));
    // Point 4: every hit zone moved by exactly the offset the content moved by,
    // so a click still lands on the section drawn under the pointer.
    expect(topsBefore.map((top, index) => top - (topsAfter[index] ?? Number.NaN))).toEqual([
      120, 120,
    ]);
    await act(async () => root.unmount());
  });

  it("leaves plain wheel as zoom over the artifact", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: SCROLL_HTML }));
    const { container, root } = await generateScrollArtifact(generate);
    const content = artifactContent(container);
    const stage = container.querySelector<HTMLElement>(".design-canvas-stage");
    const before = stage?.style.transform;

    await wheelOverArtifact(container, -120, false);

    expect(content.style.transform).toBe("translateY(0px)");
    expect(stage?.style.transform).not.toBe(before);
    await act(async () => root.unmount());
  });

  it("scrolls a selected section into the window and never moves a visible one", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: SCROLL_HTML }));
    const { container, root } = await generateScrollArtifact(generate);
    const content = artifactContent(container);

    // Far sits at page y 1400 with height 400: bottom-aligning it in an 800 px
    // window is offset 1000.
    await act(async () => {
      layerRowByName(container, "Far").click();
      await Promise.resolve();
    });
    expect(content.style.transform).toBe("translateY(-1000px)");

    // Far is selected and now visible: selecting it again must not move anything.
    await act(async () => {
      layerRowByName(container, "Far").click();
      await Promise.resolve();
    });
    expect(content.style.transform).toBe("translateY(-1000px)");

    // Top sits above the window: revealing it scrolls back to the page top.
    await act(async () => {
      layerRowByName(container, "Top").click();
      await Promise.resolve();
    });
    expect(content.style.transform).toBe("translateY(0px)");
    await act(async () => root.unmount());
  });

  it("resets the window to the top when a new artifact arrives", async () => {
    const secondHtml = "<section><h2>Second</h2></section>";
    const generate = vi
      .fn()
      .mockResolvedValueOnce({ ...GENERATION_BASE, artifactHtml: SCROLL_HTML })
      .mockResolvedValueOnce({ ...GENERATION_BASE, artifactHtml: secondHtml });
    const { container, root } = await renderDesign(createHost({ generate }));
    setCachedArtifactSections(SCROLL_HTML, [...SCROLL_SECTIONS], SCROLL_CONTENT_HEIGHT);
    setCachedArtifactSections(secondHtml, [], SCROLL_CONTENT_HEIGHT);

    await fillDraft(container, "First page.");
    const send = container.querySelector<HTMLButtonElement>(".design-generate-button");
    if (send === null) throw new Error("Generate control missing");
    await act(async () => {
      send.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    await wheelOverArtifact(container, 200);
    expect(artifactContent(container).style.transform).toBe("translateY(-200px)");

    await fillDraft(container, "Second page.");
    await act(async () => {
      send.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(artifactContent(container).style.transform).toBe("translateY(0px)");
    await act(async () => root.unmount());
  });
});

describe("revealScrollTopFor", () => {
  it("moves only when the target is outside the viewport", () => {
    // Already inside: unchanged, so a visible selection never jumps.
    expect(revealScrollTopFor(0, 100, 20, 30)).toBe(0);
    // Revealed content below the viewport pulls the list just enough.
    expect(revealScrollTopFor(0, 100, 80, 60)).toBe(40);
    // A target above the viewport scrolls back to its top.
    expect(revealScrollTopFor(200, 100, 50, 30)).toBe(50);
    // Never negative.
    expect(revealScrollTopFor(0, 100, -20, 30)).toBe(0);
  });
});

describe("DesignSurface layer list reveal", () => {
  it("scrolls the expanded row into view inside the list", async () => {
    const generate = vi.fn(async () => ({ ...GENERATION_BASE, artifactHtml: ARTIFACT_HTML }));
    const host = createHost({ generate });
    const { container, root } = await renderDesign(host);
    await generateArtifact(container, generate, "Build a shop page.");

    const list = container.querySelector<HTMLDivElement>(".design-layer-list");
    const row = layerRowByName(container, "Deals").closest<HTMLDivElement>(".design-layer-row");
    if (list === null || row === null) throw new Error("Layer list or row missing");

    let scrollTop = 0;
    Object.defineProperty(list, "scrollTop", {
      configurable: true,
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
    });
    Object.defineProperty(list, "clientHeight", { configurable: true, value: 100 });
    Object.defineProperty(list, "getBoundingClientRect", {
      configurable: true,
      value: () => ({
        top: 0,
        bottom: 100,
        left: 0,
        right: 200,
        width: 200,
        height: 100,
        x: 0,
        y: 0,
      }),
    });
    // The expanded row ends 40 px below the list viewport.
    Object.defineProperty(row, "getBoundingClientRect", {
      configurable: true,
      value: () => ({
        top: 80,
        bottom: 140,
        left: 0,
        right: 200,
        width: 200,
        height: 60,
        x: 0,
        y: 80,
      }),
    });

    await act(async () => {
      layerRowByName(container, "Deals").click();
      await Promise.resolve();
    });

    expect(scrollTop).toBe(40);
    await act(async () => root.unmount());
  });
});
