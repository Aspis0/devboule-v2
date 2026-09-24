// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  daemonStatus: vi.fn(),
  oracleAsk: vi.fn(),
  oracleFiles: vi.fn(),
  oracleStatus: vi.fn(),
  pluginsList: vi.fn(),
  providersList: vi.fn(),
  projectsList: vi.fn(),
  workspacesList: vi.fn(),
  surfaceSettingsGet: vi.fn(),
  surfaceSettingsSet: vi.fn(),
  startPresenceReporting: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  daemonStatus: mocks.daemonStatus,
  oracleAsk: mocks.oracleAsk,
  oracleFiles: mocks.oracleFiles,
  oracleStatus: mocks.oracleStatus,
  pluginsList: mocks.pluginsList,
  providersList: mocks.providersList,
  projectsList: mocks.projectsList,
  workspacesList: mocks.workspacesList,
  createSessionStateChannel: vi.fn(),
  sessionCreate: vi.fn(),
  sessionsList: vi.fn(),
  sessionsUnwatch: vi.fn(),
  sessionsWatch: vi.fn(),
  surfaceSettingsGet: mocks.surfaceSettingsGet,
  surfaceSettingsSet: mocks.surfaceSettingsSet,
}));

// The real reporter polls and invokes the window API; these tests assert the
// WIRING only — that App starts presence with the window's own state source.
vi.mock("../features/workspace/presence", () => ({
  startPresenceReporting: (...args: unknown[]) => mocks.startPresenceReporting(...args),
  reportSelection: vi.fn(),
  lookedAtSessionId: () => null,
}));

import { App } from "./App";
import {
  productionOnWindowFocusChange,
  productionWindowState,
} from "../features/workspace/attentionNotice";
import { disposeAgentHost } from "../features/design/agentHost";
import { useAppStore } from "../store/appStore";
import type { OracleIndexStatus } from "../types/ipc";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const EMPTY_INDEX = {
  state: "ready",
  indexed_files: 0,
  total_files: 0,
  indexed_chunks: 0,
  pending_files: 0,
  stale_files: 0,
  resource_budget: { max_cpu_percent: 0, max_memory_mb: 0, max_parallelism: 0 },
  model: { state: "ready" },
  reranker: null,
} as OracleIndexStatus;

function createRootContainer(): { container: HTMLDivElement; root: Root } {
  const container = document.createElement("div");
  document.body.appendChild(container);
  return { container, root: createRoot(container) };
}

async function renderDesignSurface(): Promise<{ container: HTMLDivElement; root: Root }> {
  const { container, root } = createRootContainer();
  await act(async () => root.render(<App />));
  // Mounting the whole App under full-suite parallelism can outlast the 1s
  // default. The condition is monotone — the textarea stays once rendered —
  // so a generous ceiling bounds the wait without masking anything.
  await vi.waitFor(
    () =>
      expect(
        container.querySelector<HTMLTextAreaElement>(
          'textarea[aria-label="Describe a design change"]',
        ),
      ).not.toBeNull(),
    { timeout: 10_000 },
  );
  return { container, root };
}

beforeEach(() => {
  useAppStore.setState({
    activeSurface: "design",
    plugins: null,
    installing: null,
    installError: null,
  });
  mocks.oracleAsk.mockReset();
  mocks.oracleFiles.mockReset();
  mocks.oracleStatus.mockReset();
  mocks.providersList.mockReset();
  mocks.projectsList.mockReset();
  mocks.workspacesList.mockReset();
  mocks.surfaceSettingsGet.mockReset();
  mocks.surfaceSettingsSet.mockReset();
  mocks.startPresenceReporting.mockReset();
  mocks.startPresenceReporting.mockReturnValue({
    onSelectionChanged: vi.fn(),
    dispose: vi.fn(),
  });

  mocks.surfaceSettingsGet.mockResolvedValue({ status: "absent" });
  mocks.surfaceSettingsSet.mockResolvedValue(undefined);
  mocks.oracleFiles.mockResolvedValue([]);
  mocks.daemonStatus.mockResolvedValue({ capabilities: [] });
  mocks.pluginsList.mockResolvedValue({ root: "", plugins: [], problem: null });
  mocks.providersList.mockResolvedValue({ providers: [], unreadableDirs: 0 });
  mocks.projectsList.mockResolvedValue([]);
  mocks.workspacesList.mockResolvedValue([]);
});

afterEach(async () => {
  const host = useAppStore.getState().designSession.host;
  if (host !== null) await disposeAgentHost(host);
  useAppStore.getState().clearDesignSession(host ?? undefined);
  document.body.replaceChildren();
});

describe("App Design host selection", () => {
  it("runs the real surface with an honest empty canvas when Oracle is unreachable", async () => {
    mocks.oracleStatus.mockRejectedValue(new Error("Oracle daemon unavailable"));
    const { container, root } = await renderDesignSurface();

    // The agent host starts with zero layers: no fixture nodes on the canvas.
    expect(container.querySelectorAll(".design-canvas-node")).toHaveLength(0);
    expect(container.querySelector(".design-canvas-empty")?.textContent).toContain(
      "The canvas is empty.",
    );
    // None of the demo host's invented content may leak through.
    expect(container.textContent).not.toContain("Index header");
    expect(container.textContent).not.toContain("Stale queue");
    expect(mocks.oracleStatus).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });

  it("does not fall back to fixtures when the global index simply has no files", async () => {
    mocks.oracleStatus.mockResolvedValue(EMPTY_INDEX);
    const { container, root } = await renderDesignSurface();

    expect(container.querySelectorAll(".design-canvas-node")).toHaveLength(0);
    expect(container.querySelector(".design-canvas-empty")?.textContent).toContain(
      "The canvas is empty.",
    );
    expect(container.textContent).not.toContain("Index header");
    expect(mocks.oracleStatus).not.toHaveBeenCalled();
    await act(async () => root.unmount());
  });
});

describe("App presence wiring", () => {
  it("starts presence once, on the window's own state, and disposes it", async () => {
    // The reporter is app-scope, not Workspace-scope: deleting its start, or
    // handing it the document's answer instead of the window's, fails here.
    // The placeholder surface keeps the mount synchronous and light.
    useAppStore.setState({ activeSurface: "marketplace" });
    mocks.startPresenceReporting.mockClear();
    const dispose = vi.fn();
    mocks.startPresenceReporting.mockReturnValue({
      onSelectionChanged: vi.fn(),
      dispose,
    });
    const { root } = createRootContainer();
    await act(async () => root.render(<App />));

    expect(mocks.startPresenceReporting).toHaveBeenCalledTimes(1);
    const deps = mocks.startPresenceReporting.mock.calls[0]?.[0] as
      | { windowState?: unknown; onWindowFocusChange?: unknown }
      | undefined;
    expect(deps?.windowState).toBe(productionWindowState);
    expect(deps?.onWindowFocusChange).toBe(productionOnWindowFocusChange);

    await act(async () => root.unmount());
    expect(dispose).toHaveBeenCalledTimes(1);
  });
});
