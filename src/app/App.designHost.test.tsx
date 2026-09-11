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
  reasonFromCause: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
  createSessionStateChannel: vi.fn(),
  sessionCreate: vi.fn(),
  sessionsList: vi.fn(),
  sessionsUnwatch: vi.fn(),
  sessionsWatch: vi.fn(),
  surfaceSettingsGet: mocks.surfaceSettingsGet,
  surfaceSettingsSet: mocks.surfaceSettingsSet,
}));

import { App } from "./App";
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
  await vi.waitFor(() =>
    expect(
      container.querySelector<HTMLTextAreaElement>(
        'textarea[aria-label="Describe a design change"]',
      ),
    ).not.toBeNull(),
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
