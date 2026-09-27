// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const { providersTauriMock } = await import("./providersPanelTestMocks");
  return providersTauriMock(await importOriginal());
});

vi.mock("../../workspace/workspaceSessions", async (importOriginal) => {
  const { workspaceSessionsMock } = await import("./providersPanelTestMocks");
  return workspaceSessionsMock(await importOriginal());
});

import { providerUpdate, providersList } from "../../../lib/tauri";
import { resetTerminalShellForTests } from "../providers/terminalShell";
import { takeTerminalInput } from "../../terminal/pendingTerminalInput";
import { setLastSelectedWorkspaceId } from "../../workspace/lastSelectedWorkspace";
import { useAppStore } from "../../../store/appStore";
import type { ProviderInfo, ProviderUpdateOutcome, Session } from "../../../types/ipc";
import { ProvidersPanel } from "./ProvidersPanel";
import { sessionMocks } from "./providersPanelTestMocks";
import { installedProvider, installProvidersPanelMockReset } from "./providersPanelTestSetup";

installProvidersPanelMockReset();

describe("provider update and install", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function renderPanel() {
    root = createRoot(container);
    await act(async () => root.render(<ProvidersPanel />));
    await act(async () => undefined);
  }

  function npmProvider(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
    return installedProvider({
      installChannel: "npm",
      installedVersion: "0.2.0",
      latestVersion: "0.3.0",
      npmPackage: "@vibe/grok-cli",
      tools: [],
      ...overrides,
    });
  }

  function chevron(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!button) throw new Error("row chevron did not render");
    return button;
  }

  async function openConsentFromDetails() {
    await act(async () => chevron().click());
    const update = container.querySelector<HTMLButtonElement>(".provider-update");
    if (!update) throw new Error("Update button did not render in details");
    await act(async () => update.click());
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("Consent panel did not render");
    return confirm;
  }

  it("offers Update in details and kebab only for updatable rows", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        npmProvider(),
        npmProvider({ id: "equal", latestVersion: "0.2.0" }),
        npmProvider({ id: "native", installChannel: "native" }),
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const rows = container.querySelectorAll(".prov-row-wrap");
    expect(rows).toHaveLength(3);
    rows.forEach((row) => {
      expect(row.querySelector(".prov-kebab")).not.toBeNull();
    });
    await act(async () => chevron().click());
    expect(container.querySelector(".provider-update")?.textContent).toBe("Update");
  });

  it("opens consent with the exact npm command and runs nothing until Confirm", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();
    const confirm = await openConsentFromDetails();

    expect(container.textContent).toContain("npm install -g @vibe/grok-cli@latest");
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(confirm);

    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
    expect(providerUpdate).toHaveBeenCalledWith("grok");
  });

  it("closes consent on Cancel and on Escape without running npm", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();
    await openConsentFromDetails();

    const cancel = container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("Cancel did not render");
    await act(async () => cancel.click());
    expect(providerUpdate).not.toHaveBeenCalled();

    // Cancel leaves the row expanded, so Update is still in the details.
    const update = container.querySelector<HTMLButtonElement>(".provider-update");
    if (!update) throw new Error("Update button did not stay in details");
    await act(async () => update.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("npm install -g @vibe/grok-cli@latest");
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    await act(async () => undefined);
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("npm install -g @vibe/grok-cli@latest");
  });

  it("lands focus on the row after Confirm, on both the kebab and details paths", async () => {
    async function confirmThroughKebab(): Promise<void> {
      const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
      if (!kebab) throw new Error("kebab did not render");
      await act(async () => kebab.click());
      const update = Array.from(
        container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
      ).find((item) => item.textContent === "Update");
      if (!update) throw new Error("Update item did not render");
      await act(async () => update.click());
      const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
      if (!confirm) throw new Error("consent Confirm did not render");
      await act(async () => confirm.click());
      await act(async () => undefined);
    }

    // Kebab path: the menu item is unmounted, so only the row can take focus.
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    let resolveUpdate!: (outcome: ProviderUpdateOutcome) => void;
    vi.mocked(providerUpdate).mockReturnValueOnce(
      new Promise<ProviderUpdateOutcome>((resolve) => {
        resolveUpdate = resolve;
      }),
    );
    await renderPanel();
    await confirmThroughKebab();
    expect(document.activeElement?.getAttribute("data-provider-row")).toBe("grok");
    resolveUpdate({ ok: true, exitCode: 0, log: "" });
    await act(async () => undefined);
    await act(async () => root.unmount());
    container.remove();

    // Details path: the Update button unmounts under the actions lock, so
    // the row takes focus here too instead of <body>.
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    vi.mocked(providerUpdate).mockReturnValueOnce(
      new Promise<ProviderUpdateOutcome>((resolve) => {
        resolveUpdate = resolve;
      }),
    );
    await renderPanel();
    const confirm = await openConsentFromDetails();
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(document.activeElement?.getAttribute("data-provider-row")).toBe("grok");
    resolveUpdate({ ok: true, exitCode: 0, log: "" });
    await act(async () => undefined);
  });

  it("runs npm at most once when Confirm is double-clicked", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    await renderPanel();
    const confirm = await openConsentFromDetails();

    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await act(async () => undefined);

    expect(providerUpdate).toHaveBeenCalledTimes(1);
  });

  it("replaces the catalog with the refetched list after a successful update", async () => {
    vi.mocked(providersList)
      .mockResolvedValueOnce({ providers: [npmProvider()], unreadableDirs: 0 })
      .mockResolvedValueOnce({
        providers: [npmProvider({ installedVersion: "0.3.0", latestVersion: "0.3.0" })],
        unreadableDirs: 0,
      });
    await renderPanel();
    const confirm = await openConsentFromDetails();
    await act(async () => confirm.click());
    await act(async () => undefined);

    expect(providersList).toHaveBeenCalledTimes(2);
    expect(container.textContent).toContain("up to date");
  });

  it("shows the log tail in a dismissible block when the update fails", async () => {
    const filler = "m".repeat(600);
    const log = `HEAD-MARKER ${filler} npm ERR! install crashed`;
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [npmProvider()],
      unreadableDirs: 0,
    });
    vi.mocked(providerUpdate).mockResolvedValueOnce({ ok: false, exitCode: 1, log });
    await renderPanel();
    const confirm = await openConsentFromDetails();

    await act(async () => confirm.click());
    await act(async () => undefined);

    const errorBlock = container.querySelector(".provider-update-error");
    if (!errorBlock) throw new Error("update error block did not render");
    expect(errorBlock.textContent).toContain("npm ERR! install crashed");
    expect(errorBlock.textContent).not.toContain("HEAD-MARKER");

    const dismiss = container.querySelector<HTMLButtonElement>(".provider-update-error-dismiss");
    if (!dismiss) throw new Error("dismiss did not render");
    await act(async () => dismiss.click());
    expect(container.querySelector(".provider-update-error")).toBeNull();
  });

  it("installs a not-installed row through a terminal tab, never headless npm", async () => {
    sessionMocks.create.mockResolvedValueOnce({ id: "term-1" } as Session);
    sessionMocks.creating = false;
    sessionMocks.error = null;
    resetTerminalShellForTests();
    setLastSelectedWorkspaceId("w1");
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@openai/codex",
          latestVersion: "1.2.0",
        },
      ],
      unreadableDirs: 0,
    });
    useAppStore.getState().selectSurface("settings");
    await renderPanel();

    const install = container.querySelector<HTMLButtonElement>(".provider-install");
    if (!install) throw new Error("Install did not render");
    await act(async () => install.click());
    expect(container.textContent).toContain("npm install -g @openai/codex@latest");
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("consent Confirm did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(providerUpdate).not.toHaveBeenCalled();
    expect(sessionMocks.create).toHaveBeenCalledWith("terminal", null, "w1");
    expect(takeTerminalInput("term-1")).toEqual([
      "npm install -g @openai/codex@latest; if ($? -and $LASTEXITCODE -eq 0) { codex login }",
    ]);
    expect(useAppStore.getState().activeSurface).toBe("workspace");
    setLastSelectedWorkspaceId(null);
  });
});
