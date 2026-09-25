// @vitest-environment happy-dom

import { act, type ComponentProps } from "react";
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
  throwWorkspace: false,
  throwShell: false,
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

vi.mock("../features/workspace/presence", () => ({
  startPresenceReporting: (...args: unknown[]) => mocks.startPresenceReporting(...args),
  reportSelection: vi.fn(),
  lookedAtSessionId: () => null,
}));

// The lazy Workspace resolves to a throwing component on demand: the module
// graph below App is the real one, so a fallback here proves the boundary in
// App.tsx caught it — not a mock of the boundary itself.
vi.mock("../features/workspace/Workspace", () => ({
  Workspace: () => {
    if (mocks.throwWorkspace) throw new Error("workspace render failed");
    return <section>healthy workspace</section>;
  },
}));

vi.mock("./Shell", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./Shell")>();
  function Shell(props: ComponentProps<typeof actual.Shell>) {
    if (mocks.throwShell) throw new Error("shell render failed");
    return <actual.Shell {...props} />;
  }
  return { ...actual, Shell };
});

import { App } from "./App";
import { useAppStore } from "../store/appStore";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

beforeEach(() => {
  mocks.throwWorkspace = false;
  mocks.throwShell = false;
  mocks.daemonStatus.mockReset();
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
  vi.spyOn(console, "error").mockImplementation(() => undefined);
  vi.spyOn(console, "warn").mockImplementation(() => undefined);
});

afterEach(async () => {
  useAppStore.setState({ activeSurface: "workspace" });
  document.body.replaceChildren();
  vi.restoreAllMocks();
});

describe("App error boundaries", () => {
  it("degrades one broken surface while the shell keeps working", async () => {
    useAppStore.setState({ activeSurface: "workspace" });
    mocks.throwWorkspace = true;
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root: Root = createRoot(container);
    await act(async () => root.render(<App />));
    await vi.waitFor(() => expect(container.querySelector(".surface-fallback")).not.toBeNull(), {
      timeout: 10_000,
    });

    const alert = container.querySelector(".surface-fallback") as HTMLElement;
    expect(alert.getAttribute("role")).toBe("alert");
    expect(alert.textContent).toContain("Workspace");
    expect(alert.textContent).toContain("workspace render failed");
    // The crescent shell around the broken surface keeps working.
    expect(container.querySelector('[role="navigation"]')).not.toBeNull();

    mocks.throwWorkspace = false;
    const retry = alert.querySelector<HTMLButtonElement>(".boundary-retry");
    if (retry === null) throw new Error("surface retry control did not render");
    await act(async () => retry.click());
    await vi.waitFor(() => expect(container.textContent).toContain("healthy workspace"), {
      timeout: 10_000,
    });
    await act(async () => root.unmount());
  });

  it("falls back to the root boundary when the shell itself throws", async () => {
    useAppStore.setState({ activeSurface: "pubvia" });
    mocks.throwShell = true;
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root: Root = createRoot(container);
    await act(async () => root.render(<App />));
    await vi.waitFor(
      () => expect(container.textContent).toContain("Devboule ran into a problem."),
      { timeout: 10_000 },
    );

    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("shell render failed");
    expect(alert?.querySelector(".boundary-reload")?.textContent).toBe("Reload");

    // Reload moves to the safe default surface and remounts the tree.
    mocks.throwShell = false;
    const reload = container.querySelector<HTMLButtonElement>(".boundary-reload");
    if (reload === null) throw new Error("root reload control did not render");
    await act(async () => reload.click());
    expect(useAppStore.getState().activeSurface).toBe("workspace");
    await vi.waitFor(() => expect(container.textContent).toContain("healthy workspace"), {
      timeout: 10_000,
    });
    await act(async () => root.unmount());
  });
});
