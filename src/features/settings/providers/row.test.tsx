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
      consentLines: null,
      consentNotice: null,
      consentVerb: null,
      npmFailure: null,
      terminalNotice: null,
      onDismissNotice: () => {},
      writeError: null,
      busyVerb: null,
      actionsDisabled: false,
      modelEpoch: 0,
      viaNpx: false,
      onToggleTools: () => {},
      onTurnAllOn: () => {},
      onOpenUpdate: () => {},
      loginHint: null,
      onConfirmConsent: () => {},
      onCancelConsent: () => {},
      onDismissFailure: () => {},
      onDismissWriteError: () => {},
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

  it("mounts collapsed with the name, a past-tense Started status, and no probe", async () => {
    await renderRow();
    expect(container.textContent).toContain("grok");
    expect(chevron().getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector(".prov-details")).toBeNull();
    expect(container.querySelector(".prov-dot-live")).not.toBeNull();
    expect(container.textContent).toContain("Started");
    expect(container.textContent).not.toMatch(/ready/i);
    expect(providerVocabularyGet).not.toHaveBeenCalled();
  });

  it("reads unknown authentication as idle Not started yet, never ready", async () => {
    await renderRow({ provider: providerWith({ authentication: "unknown" }) });
    expect(container.querySelector(".prov-dot-idle")).not.toBeNull();
    expect(container.querySelector(".prov-dot-live")).toBeNull();
    expect(container.textContent).toContain("Not started yet");
    expect(container.textContent).not.toMatch(/ready/i);
  });

  it("reads a failed start with the failed dot and the reason in screen-reader text", async () => {
    await renderRow({ provider: providerWith({ authentication: "failed: OAuth expired" }) });
    expect(container.querySelector(".prov-dot-failed")).not.toBeNull();
    expect(container.textContent).toContain("Start failed");
    // No aria-label on the role-less span (AT would drop it): the reason
    // travels as real text in .sr-only, which the tree exposes.
    expect(container.querySelector(".prov-status")?.getAttribute("aria-label")).toBeNull();
    expect(container.querySelector(".prov-status .sr-only")?.textContent).toContain(
      "OAuth expired",
    );
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

  it("probes an expanded row whatever its last start said, and shows no number for absent", async () => {
    await renderRow({ provider: providerWith({ authentication: "failed: gone" }) });
    await act(async () => chevron().click());
    await act(async () => undefined);
    expect(container.querySelector(".prov-details")).not.toBeNull();
    expect(providerVocabularyGet).toHaveBeenCalledTimes(1);
    expect(providerVocabularyGet).toHaveBeenCalledWith("grok", "", false);
    expect(container.textContent).not.toMatch(/\d+ models?/);
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

  it("says via npx plainly on registry rows, never on local ones", async () => {
    await renderRow({ viaNpx: true });
    expect(container.textContent).toContain("via npx");
    await renderRow({ viaNpx: false });
    expect(container.textContent).not.toContain("via npx");
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

  it("shows no Version label when no version data exists", async () => {
    await renderRow({ provider: providerWith({ installedVersion: null }) });
    await act(async () => chevron().click());
    expect(container.querySelector(".prov-details")?.textContent).not.toContain("Version");
  });

  it("shows the protocol only when known; npx provenance lives on the row", async () => {
    await renderRow({
      provider: providerWith({ protocol: null, origin: "npx-wrapper" }),
      viaNpx: true,
    });
    await act(async () => chevron().click());
    // No labelled line whose value is not a protocol; the row word carries it.
    expect(container.querySelector(".prov-details")?.textContent).not.toContain("Protocol");
    expect(container.textContent).toContain("via npx");
  });

  it("sanitises a user-declarable provider id before it becomes a DOM id", async () => {
    await renderRow({ provider: providerWith({ id: "my provider" }) });
    await act(async () => chevron().click());
    expect(chevron().getAttribute("aria-controls")).toBe("prov-details-my-provider");
    expect(container.querySelector("#prov-details-my-provider")).not.toBeNull();
  });

  it("returns focus to the kebab when its consent is cancelled", async () => {
    const onCancelConsent = vi.fn();
    await renderRow({
      provider: providerWith({
        installChannel: "npm",
        latestVersion: "0.3.0",
        npmPackage: "@vibe/grok-cli",
      }),
      onCancelConsent,
    });
    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    const update = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((item) => item.textContent === "Update");
    if (!update) throw new Error("Update item did not render");
    await act(async () => update.click());
    await act(async () =>
      root.render(
        <ProviderRow
          {...props}
          provider={providerWith({
            installChannel: "npm",
            latestVersion: "0.3.0",
            npmPackage: "@vibe/grok-cli",
          })}
          onCancelConsent={onCancelConsent}
          consentOpen
          consentLines={["npm install -g @vibe/grok-cli@latest"]}
          consentNotice={null}
          consentVerb="update"
        />,
      ),
    );
    const cancel = container.querySelector<HTMLButtonElement>(".provider-consent-cancel");
    if (!cancel) throw new Error("Cancel did not render");
    await act(async () => cancel.click());
    expect(onCancelConsent).toHaveBeenCalledTimes(1);
    expect(document.activeElement).toBe(kebab);
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
      consentLines: ["npm install -g @vibe/grok-cli@latest"],
      consentNotice: null,
      consentVerb: "update",
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

  it("shows a rejected switch write inside the row, with Dismiss", async () => {
    const sentence: ErrorSentence = {
      sentence: "A system or file operation failed on this machine.",
      detail: null,
    };
    const onDismissWriteError = vi.fn();
    await renderRow({ writeError: sentence, onDismissWriteError });
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "A system or file operation failed on this machine.",
    );
    const dismiss = container.querySelector<HTMLButtonElement>(".provider-update-error-dismiss");
    if (!dismiss) throw new Error("write-error Dismiss did not render");
    await act(async () => dismiss.click());
    expect(onDismissWriteError).toHaveBeenCalledTimes(1);
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

  it("opens login from the details button and the kebab, omitted without a handler", async () => {
    const onOpenLogin = vi.fn();
    await renderRow({ onOpenLogin });
    await act(async () => chevron().click());
    const detailsLogin = container.querySelector<HTMLButtonElement>(".provider-login");
    if (!detailsLogin) throw new Error("details Log in did not render");
    expect(detailsLogin.textContent).toBe("Log in");
    await act(async () => detailsLogin.click());
    expect(onOpenLogin).toHaveBeenCalledTimes(1);
    expect(onOpenLogin.mock.calls[0]?.[0]).toBe(detailsLogin);

    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    const menuLogin = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((item) => item.textContent === "Log in");
    if (!menuLogin) throw new Error("kebab Log in did not render");
    await act(async () => menuLogin.click());
    expect(onOpenLogin).toHaveBeenCalledTimes(2);
    expect(onOpenLogin.mock.calls[1]?.[0]).toBeNull();
  });

  it("shows no login entry points without a login handler", async () => {
    await renderRow();
    await act(async () => chevron().click());
    expect(container.querySelector(".provider-login")).toBeNull();
    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (!kebab) throw new Error("kebab did not render");
    await act(async () => kebab.click());
    expect(
      Array.from(container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]')).map(
        (item) => item.textContent,
      ),
    ).not.toContain("Log in");
  });

  it("renders every consent line verbatim with its notice", async () => {
    await renderRow({
      consentOpen: true,
      consentLines: ["npm install -g @openai/codex@latest", "codex login"],
      consentNotice: "Confirm opens a terminal tab.",
      consentVerb: "install",
      onConfirmConsent: () => {},
    });
    const lines = Array.from(container.querySelectorAll(".provider-consent-command")).map(
      (node) => node.textContent,
    );
    expect(lines).toEqual(["npm install -g @openai/codex@latest", "codex login"]);
    expect(container.textContent).toContain("Confirm opens a terminal tab.");
  });

  it("opens collapsed details for a terminal handoff, with Dismiss", async () => {
    const onDismissNotice = vi.fn();
    await renderRow({
      terminalNotice: "Install and login sent to a terminal tab — finish them there.",
      onDismissNotice,
    });
    expect(chevron().getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector('[role="status"]')?.textContent).toContain(
      "Install and login sent to a terminal tab — finish them there.",
    );
    const dismiss = container.querySelector<HTMLButtonElement>(
      '[role="status"] .provider-update-error-dismiss',
    );
    if (!dismiss) throw new Error("notice Dismiss did not render");
    await act(async () => dismiss.click());
    expect(onDismissNotice).toHaveBeenCalledTimes(1);
  });

  it("shows the login hint instead of the button once details open", async () => {
    // No auto-expand: the hint is for someone who opened details looking
    // for the login, not a reason to open every row with one by default.
    await renderRow({ loginHint: "Log in needs an open workspace." });
    expect(container.querySelector(".prov-details")).toBeNull();
    await act(async () => chevron().click());
    expect(container.querySelector(".provider-login")).toBeNull();
    expect(container.textContent).toContain("Log in needs an open workspace.");
  });

  it("shows neither button nor hint when silence is correct", async () => {
    await renderRow({ loginHint: null });
    await act(async () => chevron().click());
    expect(container.querySelector(".provider-login")).toBeNull();
    expect(container.textContent).not.toContain("Log in");
  });
});
