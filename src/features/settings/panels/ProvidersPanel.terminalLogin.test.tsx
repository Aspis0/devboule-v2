// @vitest-environment happy-dom

import { act } from "react";
import { describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const { providersTauriMock } = await import("./providersPanelTestMocks");
  return providersTauriMock(await importOriginal());
});

vi.mock("../../workspace/workspaceSessions", async (importOriginal) => {
  const { workspaceSessionsMock } = await import("./providersPanelTestMocks");
  return workspaceSessionsMock(await importOriginal());
});

import { daemonDiagnostics, providerUpdate, providersRefresh } from "../../../lib/tauri";
import { requestTerminalInput, takeTerminalInput } from "../../terminal/pendingTerminalInput";
import { recordTerminalRun, terminalRuns } from "../providers/providerTerminalRuns";
import { setLastSelectedWorkspaceId } from "../../workspace/lastSelectedWorkspace";
import { useAppStore } from "../../../store/appStore";
import { sessionMocks } from "./providersPanelTestMocks";
import { installedProvider, installProvidersPanelMockReset } from "./providersPanelTestSetup";
import {
  POSIX_LINE,
  available,
  confirm,
  consentLines,
  dom,
  listOnce,
  noteText,
  openInstall,
  renderPanel,
  installTerminalInstallDom,
} from "./providersTerminalTestHarness";

installProvidersPanelMockReset();

describe("terminal login and the handoff note", () => {
  installTerminalInstallDom();

  it("logs a documented provider in from its kebab with only the login line", async () => {
    listOnce([installedProvider({ id: "claude", executable: "claude" })]);
    await renderPanel();

    const kebab = dom.container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    const login = Array.from(
      dom.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((item) => item.textContent === "Log in");
    if (!login) throw new Error("Log in item did not render");
    await act(async () => login.click());
    await act(async () => undefined);
    // Login lines carry no shell syntax: no shell fetch gates them.
    expect(daemonDiagnostics).not.toHaveBeenCalled();
    expect(consentLines()).toEqual(["claude auth login"]);

    await confirm();
    expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
    expect(takeTerminalInput("term-1")).toEqual(["claude auth login"]);
    expect(noteText()).toContain("Login sent to a terminal tab — finish it there.");
  });

  it("names the open workspace instead of Log in where none is known", async () => {
    setLastSelectedWorkspaceId(null);
    listOnce([installedProvider({ id: "claude", executable: "claude" })]);
    await renderPanel();

    const kebab = dom.container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    expect(
      Array.from(dom.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).map(
        (item) => item.textContent,
      ),
    ).not.toContain("Log in");

    const chevron = dom.container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!chevron) throw new Error("row chevron did not render");
    await act(async () => chevron.click());
    expect(dom.container.querySelector(".provider-login")).toBeNull();
    expect(dom.container.textContent).toContain("Log in needs an open workspace.");
  });

  it("names the TUI login on an installed pi row, which the daemon can emit", async () => {
    listOnce([installedProvider({ id: "pi", executable: "pi" })]);
    await renderPanel();

    const kebab = dom.container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    expect(
      Array.from(dom.container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).map(
        (item) => item.textContent,
      ),
    ).not.toContain("Log in");

    const chevron = dom.container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!chevron) throw new Error("row chevron did not render");
    await act(async () => chevron.click());
    expect(dom.container.textContent).toContain("/login");
  });

  it("says plainly that an unknown installed id has no documented login", async () => {
    listOnce([installedProvider({ id: "my-tool", executable: "/usr/local/bin/my-tool" })]);
    await renderPanel();

    const chevron = dom.container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!chevron) throw new Error("row chevron did not render");
    await act(async () => chevron.click());
    expect(dom.container.textContent).toMatch(/no login command/i);
  });

  it("shows the daemon's own reason when the create is refused", async () => {
    sessionMocks.create.mockReset();
    sessionMocks.create.mockResolvedValue(null);
    sessionMocks.error = {
      sentence: "The workspace is unavailable.",
      detail: "it does not exist",
      workspaceId: "w1",
    };
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();

    const alert = dom.container.querySelector('[role="alert"]');
    if (!alert) throw new Error("terminal alert did not render");
    expect(alert.textContent).toContain("codex");
    expect(alert.textContent).toContain("The workspace is unavailable.");
    expect(alert.textContent).toContain("it does not exist");
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(takeTerminalInput("term-1")).toBeNull();
    expect(useAppStore.getState().activeSurface).toBe("settings");
    expect(document.activeElement?.textContent).toBe("Dismiss");

    const dismiss = dom.container.querySelector<HTMLButtonElement>(
      '[role="alert"] .provider-update-error-dismiss',
    );
    if (!dismiss) throw new Error("alert Dismiss did not render");
    await act(async () => dismiss.click());
    expect(dom.container.querySelector('[role="alert"]')).toBeNull();
  });

  function recordStaleRun(atMs: number) {
    recordTerminalRun("codex", "install", [{ label: null, text: POSIX_LINE }], "term-1", { atMs });
  }

  it("says plainly that nothing was typed past the take bound", async () => {
    requestTerminalInput("term-1", [POSIX_LINE]);
    recordStaleRun(Date.now() - 60_000);
    listOnce([available()]);
    await renderPanel();

    expect(noteText()).toContain("Nothing was typed");
    expect(noteText()).toContain(POSIX_LINE);
    const copy = dom.container.querySelector<HTMLButtonElement>(".provider-copy-button");
    if (!copy) throw new Error("copy did not render");
    await act(async () => copy.click());
  });

  it("flips a fresh handoff to never-typed once the bound passes", async () => {
    vi.useFakeTimers();
    try {
      recordStaleRun(Date.now());
      requestTerminalInput("term-1", [POSIX_LINE]);
      listOnce([available()]);
      await renderPanel();
      expect(noteText()).toContain("sent to a terminal tab");
      await act(async () => {
        vi.advanceTimersByTime(10_001);
      });
      expect(noteText()).toContain("Nothing was typed");
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the handoff note through a failed refresh, clears it on success", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();
    // The surface switch remounts the panel in production; the module store
    // is what survives it, so read back through a fresh mount.
    await act(async () => dom.root.unmount());
    listOnce([available()]);
    await renderPanel();
    expect(noteText()).toContain("sent to a terminal tab");

    vi.mocked(providersRefresh).mockRejectedValueOnce({ code: "io", message: "pipe is gone" });
    const refresh = dom.container.querySelectorAll<HTMLButtonElement>(".provider-refresh")[0];
    if (!refresh) throw new Error("Refresh did not render");
    await act(async () => refresh.click());
    await act(async () => undefined);
    expect(noteText()).toContain("sent to a terminal tab");
    expect(terminalRuns()).toHaveLength(1);

    vi.mocked(providersRefresh).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    await act(async () => refresh.click());
    await act(async () => undefined);
    expect(terminalRuns()).toEqual([]);
  });

  it("dismisses the handoff note without touching the run it names", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();

    const dismiss = dom.container.querySelector<HTMLButtonElement>(
      '[role="status"] .provider-update-error-dismiss',
    );
    if (!dismiss) throw new Error("note Dismiss did not render");
    await act(async () => dismiss.click());
    expect(dom.container.querySelector('[role="status"]')).toBeNull();
    expect(terminalRuns()).toEqual([]);
  });
});
