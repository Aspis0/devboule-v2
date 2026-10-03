// @vitest-environment happy-dom

import { act, StrictMode } from "react";
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

import {
  daemonStatus,
  providerUpdate,
  providersList,
  providersRefresh,
  providersAuthCheck,
} from "../../../lib/tauri";
import { setLastSelectedWorkspaceKey } from "../../workspace/lastSelectedWorkspace";
import type { ProviderCatalog } from "../../../types/ipc";
import { ProvidersPanel } from "./ProvidersPanel";
import {
  daemonStatusWith,
  installedProvider,
  installProvidersPanelMockReset,
} from "./providersPanelTestSetup";

installProvidersPanelMockReset();

describe("providers refresh", () => {
  it("shows unknown from an old daemon without sending the auth-check frame", async () => {
    vi.mocked(daemonStatus).mockResolvedValueOnce(daemonStatusWith(["ping", "status"]));
    await renderPanel();
    expect(providersAuthCheck).not.toHaveBeenCalled();
  });

  it("checks login on open and explicit Refresh only", async () => {
    const available = {
      id: "codex",
      executable: "",
      acpAvailable: false,
      authentication: "unknown",
      installed: false,
      npmPackage: "@openai/codex",
      latestVersion: "0.5.0",
    };
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [available], unreadableDirs: 0 });
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [available],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(providersAuthCheck).toHaveBeenCalledTimes(1);
    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    expect(providersAuthCheck).toHaveBeenCalledTimes(2);

    // An install is not a trigger: the mandate is open and explicit Refresh.
    setLastSelectedWorkspaceKey(null);
    const install = container.querySelector<HTMLButtonElement>(".provider-install");
    if (!install) throw new Error("Install did not render");
    await act(async () => install.click());
    await act(async () => undefined);
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("consent Confirm did not render");
    await act(async () => confirm.click());
    await act(async () => undefined);
    expect(providerUpdate).toHaveBeenCalledWith("codex");
    expect(providersAuthCheck).toHaveBeenCalledTimes(2);
  });

  it("never lets a stale auth check overwrite a newer catalog", async () => {
    let resolveCheck: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersAuthCheck).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveCheck = resolve;
      }),
    );
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider({ id: "grok" })],
      unreadableDirs: 0,
    });
    await renderPanel();
    // The mount check is in flight; the catalog is the mount list.
    expect(providersAuthCheck).toHaveBeenCalledTimes(1);

    // A refresh lands while the mount check is still in flight: the
    // refresh's own check answers, and the refresh's catalog wins.
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ id: "grok" })],
      unreadableDirs: 0,
    });
    vi.mocked(providersAuthCheck).mockResolvedValueOnce({
      providers: [
        installedProvider({
          id: "grok",
          authStatus: "logged_out",
          authReason: "CLI reported no active login.",
        }),
      ],
      unreadableDirs: 0,
    });
    const refresh = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!refresh) throw new Error("Refresh button did not render");
    await act(async () => refresh.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("Not logged in");

    // The mount's stale check resolves with the opposite answer: the
    // out-of-order guard must reject it.
    resolveCheck?.({
      providers: [
        installedProvider({
          id: "grok",
          authStatus: "logged_in",
          authReason: "CLI confirmed an active login.",
        }),
      ],
      unreadableDirs: 0,
    });
    await act(async () => undefined);
    expect(container.textContent).toContain("Not logged in");
    expect(container.textContent).not.toContain("Logged in");
  });

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

  async function renderPanel(strict = false) {
    root = createRoot(container);
    await act(async () =>
      root.render(
        strict ? (
          <StrictMode>
            <ProvidersPanel />
          </StrictMode>
        ) : (
          <ProvidersPanel />
        ),
      ),
    );
    await act(async () => undefined);
  }

  it("refreshes and swaps in the new catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderPanel();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(providersRefresh).toHaveBeenCalledTimes(1);
    resolveRefresh?.({
      providers: [installedProvider(), installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await act(async () => undefined);

    expect(container.textContent).toContain("fresh-cli");
  });

  it("keeps the old catalog and shows the error when refresh fails", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    vi.mocked(providersRefresh).mockRejectedValueOnce(new Error("probe timed out"));
    await renderPanel();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent ?? "").toContain(
      "probe timed out",
    );
    expect(container.textContent).toContain("grok");
  });

  it("keeps the refreshed catalog when the slow initial list resolves late", async () => {
    let resolveList: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersList).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveList = resolve;
      }),
    );
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await renderPanel();
    expect(container.textContent).toContain("Looking for agent CLIs");

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("fresh-cli");

    resolveList?.({ providers: [installedProvider()], unreadableDirs: 0 });
    await act(async () => undefined);
    expect(container.textContent).toContain("fresh-cli");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("ignores a late initial-list rejection after a refresh", async () => {
    let rejectList: ((cause: unknown) => void) | undefined;
    vi.mocked(providersList).mockReturnValueOnce(
      new Promise<ProviderCatalog>((_resolve, reject) => {
        rejectList = reject;
      }),
    );
    vi.mocked(providersRefresh).mockResolvedValueOnce({
      providers: [installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await renderPanel();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(container.textContent).toContain("fresh-cli");

    rejectList?.({ code: "internal", message: "stale list died" });
    await act(async () => undefined);
    expect(container.textContent).toContain("fresh-cli");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("ignores a second click while the refresh promise is still in flight", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderPanel();

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => {
      button.click();
      button.click();
    });
    await act(async () => undefined);

    expect(providersRefresh).toHaveBeenCalledTimes(1);
    resolveRefresh?.({ providers: [installedProvider()], unreadableDirs: 0 });
    await act(async () => undefined);
  });

  it("recovers the Refresh button after a resolve under StrictMode remount", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 0,
    });
    let resolveRefresh: ((catalog: ProviderCatalog) => void) | undefined;
    vi.mocked(providersRefresh).mockReturnValueOnce(
      new Promise<ProviderCatalog>((resolve) => {
        resolveRefresh = resolve;
      }),
    );
    await renderPanel(true);

    const button = container.querySelector<HTMLButtonElement>(".provider-refresh");
    if (!button) throw new Error("Refresh button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
    expect(button.textContent).toBe("Refreshing…");

    resolveRefresh?.({
      providers: [installedProvider(), installedProvider({ id: "fresh-cli" })],
      unreadableDirs: 0,
    });
    await act(async () => undefined);

    const done = container.querySelector<HTMLButtonElement>(".provider-refresh");
    expect(done?.textContent).toBe("Refresh");
    expect(container.textContent).toContain("fresh-cli");
  });
});
