// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import { fitViewport } from "./designViewport";
import { DesignSurface, type DesignDocument, type DesignHost } from "./DesignSurface";

const settingsMocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  loadProvider: vi.fn(),
  loadStoredProvider: vi.fn(),
  saveProvider: vi.fn(),
  loadWorkspace: vi.fn(),
  loadStoredWorkspace: vi.fn(),
  saveWorkspace: vi.fn(),
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
  name: "Panel test",
  path: "~/dev/design",
  contextPrefix: "Editing",
  draftPlaceholder: "Describe the change…",
  noContextPlaceholder: "Describe what to generate…",
  layerNotice: "Some indexed layers may be missing.",
  selectedLayerId: "oracle-panel",
  grounded: true,
  initialState: {
    zoom: 1,
    saved: false,
    draft: "",
    hiddenLayerIds: [],
  },
  layers: [
    {
      id: "oracle-panel",
      name: "OraclePanel",
      kind: "TSX",
      transform: { x: 100, y: 120, width: 300, height: 180 },
    },
  ],
  messages: [],
  workingMessage: {
    title: "Generating…",
    desc: "Reading the grounded files, then writing the node.",
  },
};

function createHost(
  document: DesignDocument = DOCUMENT,
  overrides: Partial<DesignHost> = {},
): DesignHost {
  return { loadDocument: vi.fn(async () => document), ...overrides };
}

async function renderDesign(
  designDocument: DesignDocument = DOCUMENT,
  hostOverrides: Partial<DesignHost> = {},
): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<DesignSurface host={createHost(designDocument, hostOverrides)} />);
    await Promise.resolve();
    await Promise.resolve();
  });
  return { container, root };
}

beforeEach(() => {
  useAppStore.setState({ plugins: null, installing: null, installError: null });
  settingsMocks.load.mockResolvedValue({ version: 1, mode: "all", enabledSlugs: [] });
  settingsMocks.save.mockResolvedValue(true);
  settingsMocks.loadProvider.mockResolvedValue(null);
  settingsMocks.loadStoredProvider.mockResolvedValue(null);
  settingsMocks.saveProvider.mockResolvedValue(true);
  settingsMocks.loadWorkspace.mockResolvedValue(null);
  settingsMocks.loadStoredWorkspace.mockResolvedValue(null);
  settingsMocks.saveWorkspace.mockResolvedValue(true);
  providerMocks.list.mockResolvedValue({ providers: [], unreadableDirs: 0 });
  providerMocks.daemonStatus.mockResolvedValue({ capabilities: [] });
  providerMocks.projectsList.mockResolvedValue([]);
  providerMocks.workspacesList.mockResolvedValue([]);
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("DesignSurface panels", () => {
  it("keeps the fitted bounds inside the measured canvas margin", () => {
    const margin = 80;
    const bounds = { x: 40, y: 30, w: 600, h: 300 };
    const viewport = fitViewport(bounds, 1000, 700, margin);

    const left = viewport.pan.x + bounds.x * viewport.zoom;
    const top = viewport.pan.y + bounds.y * viewport.zoom;
    const right = left + bounds.w * viewport.zoom;
    const bottom = top + bounds.h * viewport.zoom;

    expect(left).toBeGreaterThanOrEqual(margin);
    expect(top).toBeGreaterThanOrEqual(margin);
    expect(right).toBeLessThanOrEqual(1000 - margin);
    expect(bottom).toBeLessThanOrEqual(700 - margin);
  });

  it("keeps a single panel and deselects with Escape back to the canvas", async () => {
    const { container, root } = await renderDesign();
    // A selected layer marks its row: no second panel ever opens.
    expect(container.querySelector(".design-layer-row-selected")).not.toBeNull();
    expect(container.querySelector(".design-inspector-panel")).toBeNull();
    expect(container.querySelector(".design-workspace-inspector-open")).toBeNull();

    const previousBodyTabIndex = document.body.getAttribute("tabindex");
    try {
      document.body.setAttribute("tabindex", "-1");
      document.body.focus();
      expect(document.activeElement).toBe(document.body);

      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
        await Promise.resolve();
      });
    } finally {
      if (previousBodyTabIndex === null) document.body.removeAttribute("tabindex");
      else document.body.setAttribute("tabindex", previousBodyTabIndex);
    }

    expect(container.querySelector(".design-layer-row-selected")).toBeNull();
    expect(document.activeElement).toBe(container.querySelector(".design-canvas"));

    await act(async () => root.unmount());
  });

  it("keeps Escape ownership with the body, History, or craft popover", async () => {
    const { container, root } = await renderDesign(DOCUMENT, {
      generate: vi.fn(async () => ({
        sessionId: "panel-test",
        peerSessionId: null,
        createdAtMs: null,
        prompt: "panel test",
        title: "Panel test",
        desc: "Panel test",
        sources: [],
        nodeIds: [],
      })),
    });
    // The fixture document arrives with its layer selected.
    expect(container.querySelector(".design-layer-row-selected")).not.toBeNull();

    const trigger = container.querySelector<HTMLButtonElement>(
      'button[aria-controls="design-history-popover"]',
    );
    const popover = container.querySelector<HTMLDivElement>("#design-history-popover");
    if (trigger === null || popover === null) throw new Error("History controls missing");

    await act(async () => trigger.click());
    expect(popover.hidden).toBe(false);
    expect(document.activeElement).toBe(popover);

    await act(async () => {
      popover.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await Promise.resolve();
    });

    expect(popover.hidden).toBe(true);
    expect(document.activeElement).toBe(trigger);
    // The popover owned that Escape: the layer stays selected.
    expect(container.querySelector(".design-layer-row-selected")).not.toBeNull();

    const craftTrigger = container.querySelector<HTMLButtonElement>(
      'button[data-design-skill-mode-trigger="true"]',
    );
    if (craftTrigger === null) throw new Error("Craft mode trigger missing");

    await act(async () => craftTrigger.click());
    const craftPopover = container.querySelector<HTMLDivElement>("#design-skill-picker");
    if (craftPopover === null) throw new Error("Craft mode popover missing");
    expect(document.activeElement).toBe(
      craftPopover.querySelector('button[data-design-skill-mode="all"]'),
    );

    await act(async () => {
      craftPopover.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await Promise.resolve();
    });

    expect(container.querySelector("#design-skill-picker")).toBeNull();
    expect(document.activeElement).toBe(craftTrigger);
    expect(container.querySelector(".design-layer-row-selected")).not.toBeNull();

    const previousBodyTabIndex = document.body.getAttribute("tabindex");
    try {
      document.body.setAttribute("tabindex", "-1");
      document.body.focus();
      expect(document.activeElement).toBe(document.body);

      await act(async () => {
        window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
        await Promise.resolve();
      });
    } finally {
      if (previousBodyTabIndex === null) document.body.removeAttribute("tabindex");
      else document.body.setAttribute("tabindex", previousBodyTabIndex);
    }

    expect(container.querySelector(".design-layer-row-selected")).toBeNull();
    expect(document.activeElement).toBe(container.querySelector(".design-canvas"));
    await act(async () => root.unmount());
  });

  it("hides the layers panel when there are no layers", async () => {
    const emptyDocument = { ...DOCUMENT, layers: [], selectedLayerId: "" };
    const { container, root } = await renderDesign(emptyDocument);
    expect(container.querySelector(".design-layers-panel")).toBeNull();
    await act(async () => root.unmount());
  });

  it("shows the layers panel when layers exist", async () => {
    const { container, root } = await renderDesign();
    expect(container.querySelector(".design-layers-panel")).not.toBeNull();
    await act(async () => root.unmount());
  });
});
