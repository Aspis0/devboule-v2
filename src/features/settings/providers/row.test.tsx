// @vitest-environment happy-dom

// One installed provider row: chevron details, glyph, sans name, dot status,
// the single tools switch, kebab. The vocabulary probe fires only for an
// expanded row whose last start was measured.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { providerVocabularyGet } from "../../../lib/tauri";
import type { ProviderInfo } from "../../../types/ipc";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { ProviderRow, type ProviderRowProps } from "./ProviderRow";
import type { ModelCountCache } from "./ProviderModelCount";

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return { ...actual, providerVocabularyGet: vi.fn() };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function providerWith(overrides: Partial<ProviderInfo> = {}): ProviderInfo {
  return {
    id: "grok",
    executable: "C:\\grok\\grok.exe",
    acpAvailable: true,
    authentication: "ok",
    protocol: "acp",
    installedVersion: "0.2.0",
    tools: [{ name: "some_tool", description: "Something." }],
    ...overrides,
  };
}

describe("ProviderRow", () => {
  let container: HTMLDivElement;
  let root: Root;
  let cache: ModelCountCache;
  let props: ProviderRowProps;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    cache = new Map();
    props = {
      provider: providerWith(),
      toolPolicy: { enabled: true, disabledTools: [] },
      toolsDisabled: false,
      vocabularySupported: true,
      modelCache: cache,
      consentOpen: false,
      npmCommand: null,
      npmVerb: null,
      npmFailure: null,
      writeError: null,
      busyVerb: null,
      actionsDisabled: false,
      onToggleTools: () => {},
      onTurnAllOn: () => {},
      onOpenUpdate: () => {},
      onConfirmConsent: () => {},
      onCancelConsent: () => {},
      onDismissFailure: () => {},
      onRefresh: () => {},
    };
    vi.mocked(providerVocabularyGet).mockResolvedValue({
      provider: "grok",
      models: { state: "absent", items: [] },
      modes: { state: "none", items: [] },
      source: "cache",
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function renderRow(overrides: Partial<ProviderRowProps> = {}) {
    await act(async () => root.render(<ProviderRow {...props} {...overrides} />));
    await act(async () => undefined);
  }

  function chevron(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".prov-chev");
    if (!button) throw new Error("row chevron did not render");
    return button;
  }

  it("mounts collapsed with the name, a live Ready status, and no probe", async () => {
    await renderRow();
    expect(container.textContent).toContain("grok");
    expect(chevron().getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector(".prov-details")).toBeNull();
    const status = container.querySelector(".prov-dot-live");
    expect(status).not.toBeNull();
    expect(container.textContent).toContain("Ready");
    expect(providerVocabularyGet).not.toHaveBeenCalled();
  });

  it("reads unknown authentication as idle Unknown, never ready", async () => {
    await renderRow({ provider: providerWith({ authentication: "unknown" }) });
    expect(container.querySelector(".prov-dot-idle")).not.toBeNull();
    expect(container.querySelector(".prov-dot-live")).toBeNull();
    expect(container.textContent).toContain("Unknown");
    expect(container.textContent).not.toMatch(/ready/i);
  });

  it("reads a failed start with the failed dot and the reason in the accessible name", async () => {
    await renderRow({ provider: providerWith({ authentication: "failed: OAuth expired" }) });
    expect(container.querySelector(".prov-dot-failed")).not.toBeNull();
    expect(container.textContent).toContain("Start failed");
    const status = container.querySelector(".prov-status");
    expect(status?.getAttribute("aria-label")).toBe("Start failed: OAuth expired");
  });

  it("expands the measured row's details and probes its models once", async () => {
    vi.mocked(providerVocabularyGet).mockResolvedValue({
      provider: "grok",
      models: {
        state: "present",
        items: [{ modelId: "m1", name: "M1" }],
      },
      modes: { state: "none", items: [] },
      source: "cache",
    });
    await renderRow();
    await act(async () => chevron().click());
    await act(async () => undefined);
    expect(chevron().getAttribute("aria-expanded")).toBe("true");
    const details = container.querySelector(".prov-details");
    expect(details?.textContent).toContain("C:\\grok\\grok.exe");
    expect(details?.textContent).toContain("v0.2.0");
    expect(details?.textContent).toContain("ACP");
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(providerVocabularyGet).toHaveBeenCalledWith("grok", "", false);
    expect(container.textContent).toContain("1 model");
  });

  it("never probes an expanded row whose last start failed", async () => {
    await renderRow({ provider: providerWith({ authentication: "failed: gone" }) });
    await act(async () => chevron().click());
    await act(async () => undefined);
    expect(container.querySelector(".prov-details")).not.toBeNull();
    expect(providerVocabularyGet).not.toHaveBeenCalled();
  });

  it("hides the switch when there is no tool policy to show", async () => {
    await renderRow({ toolPolicy: null });
    expect(container.querySelector('[role="switch"]')).toBeNull();
  });

  it("toggles through the single switch and surfaces the legacy denials", async () => {
    const onToggleTools = vi.fn();
    const onTurnAllOn = vi.fn();
    await renderRow({
      toolPolicy: { enabled: true, disabledTools: ["old_tool"] },
      onToggleTools,
      onTurnAllOn,
    });
    const master = container.querySelector<HTMLButtonElement>('[role="switch"]');
    if (!master) throw new Error("switch did not render");
    expect(master.getAttribute("aria-checked")).toBe("true");
    expect(container.textContent).toMatch(/older setting/i);
    await act(async () => master.click());
    expect(onToggleTools).toHaveBeenCalledWith(false);
    const turnOn = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Turn all on",
    );
    if (!turnOn) throw new Error("Turn-all-on did not render");
    await act(async () => turnOn.click());
    expect(onTurnAllOn).toHaveBeenCalledTimes(1);
  });

  it("shows Updating… in auto-expanded details while its npm run is in flight", async () => {
    await renderRow({ busyVerb: "update" });
    expect(container.querySelector(".prov-details")).not.toBeNull();
    expect(container.textContent).toContain("Updating…");
  });

  it("offers no Update anywhere while another row's npm run holds the daemon", async () => {
    await renderRow({
      provider: providerWith({
        installChannel: "npm",
        latestVersion: "0.3.0",
        npmPackage: "@vibe/grok-cli",
      }),
      actionsDisabled: true,
    });
    await act(async () => chevron().click());
    expect(container.querySelector(".provider-update")).toBeNull();
    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    const names = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).map((item) => item.textContent);
    expect(names).not.toContain("Update");
  });

  it("opens Update through the kebab and renders the consent card", async () => {
    const onOpenUpdate = vi.fn();
    const onConfirmConsent = vi.fn();
    await renderRow({
      provider: providerWith({
        installChannel: "npm",
        latestVersion: "0.3.0",
        npmPackage: "@vibe/grok-cli",
      }),
      onOpenUpdate,
      consentOpen: true,
      npmCommand: "npm install -g @vibe/grok-cli@latest",
      npmVerb: "update",
      onConfirmConsent,
    });
    expect(onOpenUpdate).not.toHaveBeenCalled();
    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    const update = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((item) => item.textContent === "Update");
    if (!update) throw new Error("Update item did not render");
    await act(async () => update.click());
    expect(onOpenUpdate).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain("npm install -g @vibe/grok-cli@latest");
    const confirm = container.querySelector<HTMLButtonElement>(".provider-consent-confirm");
    if (!confirm) throw new Error("consent Confirm did not render");
    await act(async () => confirm.click());
    expect(onConfirmConsent).toHaveBeenCalledTimes(1);
  });

  it("shows a rejected switch write inside the row", async () => {
    const sentence: ErrorSentence = {
      sentence: "A system or file operation failed on this machine.",
      detail: null,
    };
    await renderRow({ writeError: sentence });
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
  });

  it("shows the npm failure with a working Dismiss", async () => {
    const onDismissFailure = vi.fn();
    await renderRow({
      npmFailure: { text: "npm ERR! crashed", detail: null },
      onDismissFailure,
    });
    expect(container.textContent).toContain("npm ERR! crashed");
    const dismiss = container.querySelector<HTMLButtonElement>(".provider-update-error-dismiss");
    if (!dismiss) throw new Error("Dismiss did not render");
    await act(async () => dismiss.click());
    expect(onDismissFailure).toHaveBeenCalledTimes(1);
  });
});
