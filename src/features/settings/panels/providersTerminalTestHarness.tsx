// Mount, fixtures and consent drivers for the Providers panel's terminal
// install and login test files.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, vi } from "vitest";

import { daemonDiagnostics, providersList } from "../../../lib/tauri";
import { useAppStore } from "../../../store/appStore";
import type { ProviderInfo, Session } from "../../../types/ipc";
import { clearTerminalInputForTests } from "../../terminal/pendingTerminalInput";
import { setLastSelectedWorkspaceId } from "../../workspace/lastSelectedWorkspace";
import { clearTerminalRuns } from "../providers/providerTerminalRuns";
import { resetTerminalShellForTests } from "../providers/terminalShell";
import { ProvidersPanel } from "./ProvidersPanel";
import { sessionMocks } from "./providersPanelTestMocks";

export const WINDOWS_OS = "Windows 10.0.26200 (x86_64)";
export const POSIX_OS = "linux (x86_64)";
export const POWERSHELL_LINE =
  "npm install -g @openai/codex@latest; if ($? -and $LASTEXITCODE -eq 0) { codex login }";
export const POSIX_LINE = "npm install -g @openai/codex@latest && codex login";

/** The mounted test surface; tests read it directly. */
export const dom: { container: HTMLDivElement; root: Root } = {
  container: undefined as unknown as HTMLDivElement,
  root: undefined as unknown as Root,
};

export function installTerminalInstallDom() {
  beforeEach(() => {
    dom.container = document.createElement("div");
    document.body.appendChild(dom.container);
    clearTerminalRuns();
    resetTerminalShellForTests();
    sessionMocks.create.mockReset();
    sessionMocks.create.mockResolvedValue({ id: "term-1" } as Session);
    sessionMocks.creating = false;
    sessionMocks.error = null;
    vi.mocked(daemonDiagnostics).mockReset();
    vi.mocked(daemonDiagnostics).mockResolvedValue({
      environment: { osVersion: WINDOWS_OS },
    } as never);
    setLastSelectedWorkspaceId("w1");
    useAppStore.getState().selectSurface("settings");
  });

  afterEach(async () => {
    await act(async () => dom.root.unmount());
    dom.container.remove();
    vi.clearAllMocks();
    clearTerminalRuns();
    clearTerminalInputForTests();
    resetTerminalShellForTests();
    setLastSelectedWorkspaceId(null);
    useAppStore.getState().selectSurface("workspace");
  });
}

export async function renderPanel() {
  dom.root = createRoot(dom.container);
  await act(async () => dom.root.render(<ProvidersPanel />));
  await act(async () => undefined);
}

export function available(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "codex",
    executable: "",
    acpAvailable: false,
    authentication: "unknown",
    installed: false,
    npmPackage: "@openai/codex",
    latestVersion: "0.5.0",
    ...overrides,
  };
}

export function listOnce(providers: ProviderInfo[]) {
  vi.mocked(providersList).mockResolvedValueOnce({ providers, unreadableDirs: 0 });
}

export function consentLines(): string[] {
  return Array.from(dom.container.querySelectorAll(".provider-consent-command")).map(
    (node) => node.textContent ?? "",
  );
}

export async function confirm() {
  const button = dom.container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
  if (!button) throw new Error("consent Confirm did not render");
  await act(async () => button.click());
  await act(async () => undefined);
}

export async function openInstall() {
  const install = dom.container.querySelector<HTMLButtonElement>(".provider-install");
  if (!install) throw new Error("Install did not render");
  await act(async () => install.click());
  // The shell report resolves between open and assert.
  await act(async () => undefined);
}

export function noteText(): string {
  return dom.container.querySelector('[role="status"]')?.textContent ?? "";
}
