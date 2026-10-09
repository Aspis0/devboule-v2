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

import { providersList } from "../../../lib/tauri";
import { ProvidersPanel } from "./ProvidersPanel";
import { installedProvider, installProvidersPanelMockReset } from "./providersPanelTestSetup";

installProvidersPanelMockReset();

describe("providers sections and search", () => {
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

  /** Drive the controlled search box the way a human does: React reads the
   *  native value setter, so a plain property assignment would be ignored. */
  function typeSearch(box: HTMLInputElement, text: string) {
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (!setter) throw new Error("no native value setter");
    setter.call(box, text);
    box.dispatchEvent(new Event("input", { bubbles: true }));
  }

  it("splits installed rows from an Available to install section", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider(),
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
          latestVersion: "1.2.0",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(container.textContent).toContain("Installed");
    expect(container.textContent).toContain("Available to install");
    const installedSection = Array.from(container.querySelectorAll("section")).find((section) =>
      section.textContent?.includes("Installed"),
    );
    expect(installedSection?.textContent).toContain("grok");
    expect(installedSection?.textContent).not.toContain("codex-acp");
  });

  it("renders a not-installed row with its package, version, and accent Install", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
          latestVersion: "1.2.0",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    expect(container.textContent).toContain("@agentclientprotocol/codex-acp");
    expect(container.textContent).toContain("v1.2.0 available");
    expect(container.querySelector(".provider-install")?.textContent).toBe("Install");
    expect(container.textContent).not.toContain("authentication unknown");
  });

  it("renders no invented prose on available rows: every word is data", async () => {
    // ProviderInfo carries no description field, so an available row may
    // only show its id, its package, its version line, and Install. Strip
    // those known strings: whatever text remains is invented.
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
          latestVersion: "1.2.0",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const row = container.querySelector(".prov-available-row");
    if (!row) throw new Error("available row did not render");
    const known = ["codex-acp", "@agentclientprotocol/codex-acp", "v1.2.0 available", "Install"];
    let rest = row.textContent ?? "";
    for (const word of known) rest = rest.replace(word, "");
    expect(rest.trim()).toBe("");
    expect(row.querySelector(".prov-description")).toBeNull();
  });

  it("filters the available rows by name through the catalogue search", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider(),
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
        {
          id: "forge-runner",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@vibe/forge",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const search = container.querySelector<HTMLInputElement>('input[type="search"]');
    if (!search) throw new Error("catalogue search did not render");
    await act(async () => typeSearch(search, "codex"));
    await act(async () => undefined);

    expect(container.textContent).toContain("codex-acp");
    expect(container.textContent).not.toContain("forge-runner");
    expect(container.textContent).toContain("grok");
  });

  it("says so when no available row matches the search", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const search = container.querySelector<HTMLInputElement>('input[type="search"]');
    if (!search) throw new Error("catalogue search did not render");
    await act(async () => typeSearch(search, "zzz-no-such-provider"));
    await act(async () => undefined);

    expect(container.textContent).toMatch(/no providers match/i);
    expect(container.textContent).not.toContain("codex-acp");
  });

  it("groups npx runners after the real CLIs, saying via npx on each row", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [
        installedProvider({ id: "grok" }),
        {
          id: "agoragentic-acp",
          executable: "agoragentic-mcp@1.3.0",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
          origin: "npx-wrapper",
        },
        {
          id: "codex-acp",
          executable: "",
          acpAvailable: false,
          authentication: "unknown",
          installed: false,
          npmPackage: "@agentclientprotocol/codex-acp",
        },
      ],
      unreadableDirs: 0,
    });
    await renderPanel();

    const installed = Array.from(container.querySelectorAll("section")).find(
      (section) => section.getAttribute("aria-label") === "Installed",
    );
    const npx = Array.from(container.querySelectorAll("section")).find(
      (section) => section.getAttribute("aria-label") === "Run on demand",
    );
    if (!installed) throw new Error("Installed section did not render");
    if (!npx) throw new Error("npx section did not render");
    expect(installed.textContent).toContain("grok");
    expect(installed.textContent).not.toContain("agoragentic-acp");
    expect(installed.textContent).not.toContain("via npx");
    expect(npx.textContent).toContain("agoragentic-acp");
    expect(npx.textContent).toContain("via npx");
    // The explanation sits under the section's Advanced, not on the page.
    expect(npx.querySelector("[data-settings-advanced]")?.textContent).toMatch(
      /nothing is installed/i,
    );
    expect(npx.querySelector(":scope > p")).toBeNull();
    // Install flow untouched: the not-installed row stays available.
    expect(container.textContent).toContain("Available to install");
    // Order: Installed, then Available with its search, then the long npx
    // card last — the search must not sit under 20 rows.
    const order = Array.from(container.querySelectorAll("#settings-panel-providers > section")).map(
      (section) => section.getAttribute("aria-label"),
    );
    expect(order).toEqual(["Installed", "Available to install", "Run on demand"]);
  });

  it("says when no agent CLI is on PATH", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({ providers: [], unreadableDirs: 0 });
    await renderPanel();

    expect(container.textContent).toContain("No agent CLI found on PATH");
    // The install hint is explanation: it waits under Advanced.
    expect(
      container.querySelector(".provider-empty [data-settings-advanced]")?.textContent,
    ).toContain("Install an agent CLI");
  });

  it("notes unreadable PATH directories under a non-empty catalog", async () => {
    vi.mocked(providersList).mockResolvedValueOnce({
      providers: [installedProvider()],
      unreadableDirs: 2,
    });
    await renderPanel();

    expect(container.textContent).toContain("grok");
    expect(container.textContent).toContain("2 PATH directories could not be read");
  });
});
