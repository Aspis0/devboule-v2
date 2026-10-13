// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore } from "../../store/appStore";
import { DesignSurface, type DesignDocument, type DesignHost } from "./DesignSurface";
import type { DesignMessage } from "./designHost";

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
  createSessionStateChannel: vi.fn(),
  sessionCreate: vi.fn(),
  sessionsList: vi.fn(),
  sessionsUnwatch: vi.fn(),
  sessionsWatch: vi.fn(),
}));

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const ARCADE_HTML = '<main class="arcade">Neon arcade</main>';
const BRIEF_HTML = '<main class="brief">Neon brief</main>';

function result(id: string, title: string, artifactHtml: string): DesignMessage {
  return {
    id,
    role: "assistant",
    status: "done",
    title,
    desc: "Finished by a Design agent.",
    sources: [],
    nodeIds: [],
    artifactHtml,
  };
}

const PROMPT: DesignMessage = {
  id: "prompt-0",
  role: "user",
  text: "Give me two of them.",
};

/** Two finished results, the transcript the switcher is built for. */
const TWO_RESULTS: DesignMessage[] = [
  PROMPT,
  result("delegated-design-child-arcade", "Neon arcade", ARCADE_HTML),
  result("delegated-design-child-brief", "Neon brief", BRIEF_HTML),
];

function documentWith(messages: readonly DesignMessage[]): DesignDocument {
  return {
    name: "Switcher test",
    path: "~/dev/design",
    contextPrefix: "Editing",
    draftPlaceholder: "Describe the change…",
    noContextPlaceholder: "Describe what to generate…",
    selectedLayerId: "",
    grounded: true,
    initialState: { zoom: 1, saved: false, draft: "", hiddenLayerIds: [] },
    layers: [],
    messages,
    workingMessage: { title: "Generating…", desc: "Asking the agent." },
  };
}

function createHost(document: DesignDocument): DesignHost {
  return { loadDocument: vi.fn(async () => document) };
}

async function renderDesign(messages: readonly DesignMessage[]): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
  host: DesignHost;
}> {
  const host = createHost(documentWith(messages));
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(<DesignSurface host={host} />);
    await Promise.resolve();
    await Promise.resolve();
  });
  return { container, root, host };
}

function cards(container: HTMLElement): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll<HTMLButtonElement>(".design-artifact-card"));
}

function cardTitles(container: HTMLElement): (string | null)[] {
  return cards(container).map((card) => card.textContent);
}

function messageIds(): string[] {
  return useAppStore.getState().designSession.messages.map((message) => message.id);
}

function artifactOnCanvas(container: HTMLElement): string {
  const frame = container.querySelector<HTMLIFrameElement>(".design-artifact-frame");
  return frame?.getAttribute("srcdoc") ?? "";
}

const DESIGN_FOLDER_PROJECT = {
  id: "project-switcher",
  name: "Switcher folder",
  path: "C:/switcher",
} as const;
const DESIGN_FOLDER = {
  id: "workspace-switcher",
  projectId: DESIGN_FOLDER_PROJECT.id,
  title: "main workspace",
  isolation: "local",
  path: "C:/switcher",
} as const;

beforeEach(() => {
  useAppStore.setState({ plugins: null, installing: null, installError: null });
  settingsMocks.load.mockResolvedValue({ version: 1, mode: "all", enabledSlugs: [] });
  settingsMocks.save.mockResolvedValue(true);
  settingsMocks.loadProvider.mockResolvedValue(null);
  settingsMocks.loadStoredProvider.mockResolvedValue(null);
  settingsMocks.saveProvider.mockResolvedValue(true);
  settingsMocks.loadWorkspace.mockResolvedValue(DESIGN_FOLDER.id);
  settingsMocks.loadStoredWorkspace.mockResolvedValue(null);
  settingsMocks.saveWorkspace.mockResolvedValue(true);
  settingsMocks.loadOutput.mockResolvedValue("page");
  settingsMocks.saveOutput.mockResolvedValue(true);
  providerMocks.list.mockResolvedValue({ providers: [], unreadableDirs: 0 });
  providerMocks.daemonStatus.mockResolvedValue({ capabilities: [] });
  providerMocks.projectsList.mockResolvedValue([DESIGN_FOLDER_PROJECT]);
  providerMocks.workspacesList.mockResolvedValue([DESIGN_FOLDER]);
});

afterEach(() => {
  document.body.replaceChildren();
});

describe("the canvas toolbar's artifact switcher", () => {
  it("names every finished result and marks the one the canvas shows", async () => {
    const { container, root } = await renderDesign(TWO_RESULTS);

    expect(cardTitles(container)).toEqual(["Neon arcade", "Neon brief"]);
    const [arcade, brief] = cards(container);
    expect(arcade?.getAttribute("aria-pressed")).toBe("false");
    expect(arcade?.className).not.toContain("design-artifact-card-shown");
    expect(brief?.getAttribute("aria-pressed")).toBe("true");
    expect(brief?.className).toContain("design-artifact-card-shown");

    await act(async () => root.unmount());
  });

  it("shows the clicked result by moving its card to the end of the transcript", async () => {
    const { container, root } = await renderDesign(TWO_RESULTS);
    expect(artifactOnCanvas(container)).toContain(BRIEF_HTML);

    const arcade = cards(container)[0];
    if (arcade === undefined) throw new Error("Neon arcade card missing");
    await act(async () => {
      arcade.click();
    });

    expect(messageIds()).toEqual([
      "prompt-0",
      "delegated-design-child-brief",
      "delegated-design-child-arcade",
    ]);
    expect(useAppStore.getState().designSession.latestArtifact?.html).toBe(ARCADE_HTML);
    expect(artifactOnCanvas(container)).toContain(ARCADE_HTML);
    // The card that is now in front says so, without a selection of its own.
    expect(cardTitles(container)).toEqual(["Neon brief", "Neon arcade"]);
    expect(cards(container)[1]?.getAttribute("aria-pressed")).toBe("true");

    await act(async () => root.unmount());
  });

  it("refuses while a generation runs, so the settling result keeps the canvas", async () => {
    const { container, root, host } = await renderDesign(TWO_RESULTS);
    const before = messageIds();

    await act(async () => {
      useAppStore.getState().setDesignGeneration(host, {
        assistantId: "live",
        controller: new AbortController(),
      });
    });

    const disabled = cards(container);
    expect(disabled.length).toBe(2);
    expect(disabled.map((card) => card.disabled)).toEqual([true, true]);

    await act(async () => {
      disabled[0]?.click();
    });

    expect(messageIds()).toEqual(before);
    expect(useAppStore.getState().designSession.latestArtifact?.html).toBe(BRIEF_HTML);
    expect(artifactOnCanvas(container)).toContain(BRIEF_HTML);

    await act(async () => root.unmount());
  });

  it("renders nothing when one result is all the canvas holds", async () => {
    const { container, root } = await renderDesign([PROMPT, TWO_RESULTS[1]!]);

    expect(container.querySelector(".design-artifact-switcher")).toBeNull();

    await act(async () => root.unmount());
  });

  it("never names a message that carries no artifact", async () => {
    const withoutArtifact: DesignMessage = {
      id: "delegated-design-child-quiet",
      role: "assistant",
      status: "done",
      title: "Neon nothing",
      desc: "Finished by a Design agent.",
      sources: [],
      nodeIds: [],
    };
    const { container, root } = await renderDesign([...TWO_RESULTS, withoutArtifact]);

    expect(cardTitles(container)).toEqual(["Neon arcade", "Neon brief"]);

    await act(async () => root.unmount());
  });
});
