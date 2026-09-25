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

vi.mock("../lib/tauri", () => ({
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
// graph below AppRoot is the real one, so a fallback here proves the boundary
// above App caught it — not a mock of the boundary itself.
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

import { AppRoot } from "./AppRoot";
import { useAppStore } from "../store/appStore";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

const roots: Root[] = [];

function mountApp(): HTMLDivElement {
  const container = document.createElement("div");
  document.body.appendChild(container);
  roots.push(createRoot(container));
  return container;
}

function stubDocumentReload(): ReturnType<typeof vi.fn> {
  const reload = vi.fn();
  Object.defineProperty(window.location, "reload", { configurable: true, value: reload });
  return reload;
}

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
  for (const root of roots.splice(0)) await act(async () => root.unmount());
  useAppStore.setState({ activeSurface: "workspace" });
  document.body.replaceChildren();
  const location = window.location as unknown as Record<string, unknown>;
  if (Object.hasOwn(location, "reload")) delete location.reload;
  vi.restoreAllMocks();
});

describe("App error boundaries", () => {
  it("degrades one broken surface while the shell keeps working", async () => {
    useAppStore.setState({ activeSurface: "workspace" });
    mocks.throwWorkspace = true;
    const container = mountApp();
    await act(async () => roots[roots.length - 1]?.render(<AppRoot />));
    await vi.waitFor(() => expect(container.querySelector(".surface-fallback")).not.toBeNull(), {
      timeout: 10_000,
    });

    const alert = container.querySelector(".surface-fallback") as HTMLElement;
    expect(alert.getAttribute("role")).toBe("alert");
    expect(alert.textContent).toContain("Workspace");
    expect(alert.textContent).toContain("workspace render failed");
    // The crescent shell around the broken surface keeps working.
    expect(container.querySelector('[role="navigation"]')).not.toBeNull();
    // The IPC stubs really intercept: the shell's inventory read went
    // through the mock, not the real daemon module.
    expect(mocks.pluginsList).toHaveBeenCalled();

    mocks.throwWorkspace = false;
    const retry = alert.querySelector<HTMLButtonElement>(".boundary-retry");
    if (retry === null) throw new Error("surface retry control did not render");
    await act(async () => retry.click());
    await vi.waitFor(() => expect(container.textContent).toContain("healthy workspace"), {
      timeout: 10_000,
    });
  });

  it("catches a throw in App's own effects above App", async () => {
    // The roster watch and the presence reporter run in App's effects; an
    // error there is attributed to App's fiber, so only a boundary above App
    // can catch it — with the boundary in its old place inside App, React
    // unmounts the root and this render throws out.
    useAppStore.setState({ activeSurface: "pubvia" });
    mocks.startPresenceReporting.mockImplementation(() => {
      throw new Error("presence failed");
    });
    const container = mountApp();
    await act(async () => roots[roots.length - 1]?.render(<AppRoot />));
    await vi.waitFor(
      () => expect(container.textContent).toContain("Devboule ran into a problem."),
      { timeout: 10_000 },
    );
    expect(container.textContent).toContain("presence failed");
  });

  it("falls back to the root boundary when the shell itself throws", async () => {
    useAppStore.setState({ activeSurface: "pubvia" });
    mocks.throwShell = true;
    const reload = stubDocumentReload();
    const container = mountApp();
    await act(async () => roots[roots.length - 1]?.render(<AppRoot />));
    await vi.waitFor(
      () => expect(container.textContent).toContain("Devboule ran into a problem."),
      { timeout: 10_000 },
    );

    const alert = container.querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("shell render failed");
    expect(alert?.querySelector(".root-fallback-footer .boundary-reload")?.textContent).toBe(
      "Reload",
    );

    // The shell is healthy again, so the button's document reload is the
    // recovery under test.
    mocks.throwShell = false;
    const button = container.querySelector<HTMLButtonElement>(".boundary-reload");
    if (button === null) throw new Error("root reload control did not render");
    await act(async () => button.click());
    expect(reload).toHaveBeenCalledTimes(1);
  });
});
