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

import { daemonDiagnostics, providerUpdate, providersList } from "../../../lib/tauri";
import { takeTerminalInput } from "../../terminal/pendingTerminalInput";
import { terminalRuns } from "../providers/providerTerminalRuns";
import { setLastSelectedWorkspaceId } from "../../workspace/lastSelectedWorkspace";
import { useAppStore } from "../../../store/appStore";
import type { Session } from "../../../types/ipc";
import { sessionMocks } from "./providersPanelTestMocks";
import { installProvidersPanelMockReset } from "./providersPanelTestSetup";
import {
  POSIX_LINE,
  POSIX_OS,
  POWERSHELL_LINE,
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

describe("terminal install", () => {
  installTerminalInstallDom();

  it("shows the PowerShell gated line in consent and types exactly that line", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    // The consent is the contract: what the person saw is what the tab
    // receives — read back through the take, not the plan.
    expect(consentLines()).toEqual([POWERSHELL_LINE]);
    expect(dom.container.textContent).toContain("terminal tab");
    expect(dom.container.textContent).toContain("without your shell profile");
    expect(providerUpdate).not.toHaveBeenCalled();

    await confirm();
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(sessionMocks.create).toHaveBeenCalledTimes(1);
    expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
    expect(takeTerminalInput("term-1")).toEqual([POWERSHELL_LINE]);
    expect(useAppStore.getState().activeSurface).toBe("workspace");
    expect(terminalRuns().map((run) => ({ providerId: run.providerId, verb: run.verb }))).toEqual([
      { providerId: "codex", verb: "install" },
    ]);
    expect(noteText()).toContain("Install and login sent to a terminal tab — finish them there.");
  });

  it("shows the POSIX line when the daemon reports a POSIX OS", async () => {
    vi.mocked(daemonDiagnostics).mockReset();
    vi.mocked(daemonDiagnostics).mockResolvedValue({
      environment: { osVersion: POSIX_OS },
    } as never);
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    expect(consentLines()).toEqual([POSIX_LINE]);
    await confirm();
    expect(takeTerminalInput("term-1")).toEqual([POSIX_LINE]);
  });

  it("waits for the shell report with no Confirm, and Cancel runs nothing", async () => {
    vi.mocked(daemonDiagnostics).mockReset();
    vi.mocked(daemonDiagnostics).mockReturnValueOnce(new Promise(() => {}) as never);
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    expect(dom.container.textContent).toContain("Checking which shell");
    expect(dom.container.querySelector(".provider-consent-confirm")).toBeNull();
    const cancel = dom.container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("waiting Cancel did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);
    expect(sessionMocks.create).not.toHaveBeenCalled();
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(terminalRuns()).toEqual([]);
  });

  it("copies instead of typing when the shell cannot be confirmed", async () => {
    const writes: string[] = [];
    vi.stubGlobal("navigator", {
      ...navigator,
      clipboard: {
        writeText: vi.fn(async (text: string) => {
          writes.push(text);
        }),
      },
    });
    try {
      vi.mocked(daemonDiagnostics).mockReset();
      vi.mocked(daemonDiagnostics).mockRejectedValueOnce(new Error("daemon unreachable"));
      listOnce([available()]);
      await renderPanel();
      await openInstall();

      expect(dom.container.textContent).toContain("could not be confirmed");
      expect(consentLines()).toEqual([POWERSHELL_LINE, POSIX_LINE]);
      const labels = Array.from(dom.container.querySelectorAll(".provider-copy-label")).map(
        (node) => node.textContent,
      );
      expect(labels).toEqual(["Windows PowerShell", "POSIX shells"]);
      const copies = Array.from(
        dom.container.querySelectorAll<HTMLButtonElement>(".provider-copy-button"),
      );
      expect(copies).toHaveLength(2);
      await act(async () => copies[0]?.click());
      expect(writes).toEqual([POWERSHELL_LINE]);
      expect(copies[0]?.textContent).toBe("Copied");

      await confirm();
      // The tab opens untyped: nothing was requested for it.
      expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
      expect(takeTerminalInput("term-1")).toBeNull();
      expect(noteText()).toContain("paste the copied line");
      expect(noteText()).toContain(POSIX_LINE);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("runs nothing until Confirm, and Cancel runs nothing at all", async () => {
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    expect(sessionMocks.create).not.toHaveBeenCalled();

    const cancel = dom.container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("consent Cancel did not render");
    await act(async () => cancel.click());
    await act(async () => undefined);
    expect(sessionMocks.create).not.toHaveBeenCalled();
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(terminalRuns()).toEqual([]);
    expect(useAppStore.getState().activeSurface).toBe("settings");
  });

  it("says the in-flight create plainly and never sends it twice", async () => {
    sessionMocks.creating = true;
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    await confirm();

    // The controller dropped the create: the daemon was never asked, so
    // the page must not print the daemon-refusal sentence.
    expect(sessionMocks.create).not.toHaveBeenCalled();
    const alert = dom.container.querySelector('[role="alert"]');
    if (!alert) throw new Error("terminal alert did not render");
    expect(alert.textContent).toContain("already starting");
    expect(alert.textContent).not.toContain("did not start one");
    expect(useAppStore.getState().activeSurface).toBe("settings");
  });

  it("does not yank the person back when the create outlives the panel", async () => {
    let resolveCreate!: (session: Session) => void;
    sessionMocks.create.mockReset();
    sessionMocks.create.mockReturnValueOnce(
      new Promise<Session>((resolve) => {
        resolveCreate = resolve;
      }),
    );
    listOnce([available()]);
    await renderPanel();
    await openInstall();
    const button = dom.container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!button) throw new Error("consent Confirm did not render");
    await act(async () => button.click());
    // The person moves on while the daemon spawn is in flight.
    useAppStore.getState().selectSurface("design");
    await act(async () => {
      resolveCreate({ id: "term-9" } as Session);
    });
    await act(async () => undefined);

    expect(useAppStore.getState().activeSurface).toBe("design");
    expect(takeTerminalInput("term-9")).toHaveLength(1);
    expect(terminalRuns().map((run) => run.providerId)).toEqual(["codex"]);
  });

  it("installs headlessly when no workspace is open, with no login step", async () => {
    setLastSelectedWorkspaceId(null);
    listOnce([available()]);
    await renderPanel();
    await openInstall();

    expect(consentLines()).toEqual(["npm install -g @openai/codex@latest"]);
    expect(dom.container.textContent).toContain("No workspace is open");
    expect(dom.container.textContent).toContain("no login step");
    expect(dom.container.textContent).toContain("Log in on the installed row");
    expect(dom.container.textContent).not.toContain("opens a terminal tab");

    await confirm();
    expect(sessionMocks.create).not.toHaveBeenCalled();
    expect(providerUpdate).toHaveBeenCalledWith("codex");
    expect(vi.mocked(providersList).mock.calls.length).toBeGreaterThan(1);
    expect(useAppStore.getState().activeSurface).toBe("settings");
  });

  it("offers no Install button for a package outside the strict name shape", async () => {
    listOnce([available({ id: "codex", npmPackage: "x; calc" })]);
    await renderPanel();

    expect(dom.container.querySelector(".provider-install")).toBeNull();
  });
});
