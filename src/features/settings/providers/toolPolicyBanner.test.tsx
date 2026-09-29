// @vitest-environment happy-dom

// The failed-closed banner: it shows the daemon's reason verbatim, never a
// rewritten claim, with the one remedy and a restart button behind a confirm.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { ask } from "@tauri-apps/plugin-dialog";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonRestart } from "../../../lib/tauri";
import { assembleCssProof, removeCssProof } from "../../workspace/cssProof";
import { ToolPolicyBanner } from "./ToolPolicyBanner";

vi.mock("@tauri-apps/plugin-dialog", () => ({ ask: vi.fn() }));
vi.mock("../../../lib/tauri", () => ({ daemonRestart: vi.fn() }));

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("ToolPolicyBanner", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    vi.mocked(ask).mockResolvedValue(true);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function renderBanner(reason: string): Promise<Element> {
    await act(async () => root.render(<ToolPolicyBanner reason={reason} />));
    const banner = container.querySelector('[role="alert"]');
    if (!banner) throw new Error("failed-closed banner did not render");
    return banner;
  }

  function restartButton(): HTMLButtonElement {
    const button = Array.from(container.querySelectorAll("button")).find(
      (candidate) => candidate.textContent === "Restart daemon",
    );
    if (!button) throw new Error("restart button did not render");
    return button as HTMLButtonElement;
  }

  it("shows the daemon's reason verbatim without rewriting the cause", async () => {
    const banner = await renderBanner("tool-policies.json exists but holds no policy");
    expect(banner.textContent).toContain("tool-policies.json exists but holds no policy");
    expect(banner.textContent).not.toContain("unreadable");
    expect(banner.textContent).not.toContain("are off");
    expect(banner.textContent).toContain("restart");
  });

  it("asks first and restarts only on confirm", async () => {
    await renderBanner("tool-policies.json exists but holds no policy");
    await act(async () => restartButton().click());
    expect(ask).toHaveBeenCalledTimes(1);
    expect(daemonRestart).toHaveBeenCalledTimes(1);
  });

  it("never restarts on cancel", async () => {
    vi.mocked(ask).mockResolvedValue(false);
    await renderBanner("tool-policies.json exists but holds no policy");
    await act(async () => restartButton().click());
    expect(ask).toHaveBeenCalledTimes(1);
    expect(daemonRestart).not.toHaveBeenCalled();
  });

  it("shows the command's own sentence when the restart fails", async () => {
    // bridge.client() None rides here as { code: io, message }: the banner
    // introduces that sentence verbatim instead of rewriting it.
    vi.mocked(daemonRestart).mockRejectedValueOnce({
      code: "io",
      message: "The daemon connection was lost.",
    });
    await renderBanner("tool-policies.json exists but holds no policy");
    await act(async () => restartButton().click());
    await act(async () => undefined);
    expect(daemonRestart).toHaveBeenCalledTimes(1);
    const banner = container.querySelector('[role="alert"]');
    expect(banner?.textContent).toContain("Restart failed: The daemon connection was lost.");
  });

  it("says the confirmation did not open when the dialog fails", async () => {
    vi.mocked(ask).mockRejectedValueOnce(new Error("dialog unavailable"));
    await renderBanner("tool-policies.json exists but holds no policy");
    await act(async () => restartButton().click());
    await act(async () => undefined);
    expect(daemonRestart).not.toHaveBeenCalled();
    const banner = container.querySelector('[role="alert"]');
    expect(banner?.textContent).toContain("The confirmation did not open.");
  });

  it("locks the button while the restart is in flight", async () => {
    let release!: () => void;
    vi.mocked(daemonRestart).mockImplementationOnce(
      () => new Promise<void>((resolve) => (release = resolve)),
    );
    await renderBanner("tool-policies.json exists but holds no policy");
    const button = restartButton();
    expect(button.disabled).toBe(false);
    button.click();
    await act(async () => undefined);
    expect(button.disabled).toBe(true);
    expect(button.textContent).toContain("Restarting");
    await act(async () => release());
    expect(button.disabled).toBe(false);
    expect(daemonRestart).toHaveBeenCalledTimes(1);
  });
});

// The banner's alert face against the real stylesheets in bundle order:
// tokens, global, providers (the banner's sheet) before settings (the
// button's sheet). Light theme only; the dark theme belongs to a live check.
describe("ToolPolicyBanner styles (real stylesheets, no app launch)", () => {
  const rootDir = resolve(import.meta.dirname, "../../../..");
  const read = (path: string): string => readFileSync(resolve(rootDir, path), "utf8");
  const proof = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/providers.css"),
    read("src/features/settings/settings.css"),
  ]);

  afterEach(() => {
    removeCssProof();
    document.body.innerHTML = "";
  });

  function styledBox(className: string): HTMLElement {
    const el = document.createElement("div");
    el.className = className;
    document.body.appendChild(el);
    return el;
  }

  it("gives every banner class a rule", () => {
    for (const selector of [
      ".prov-policy-banner",
      ".prov-policy-banner-title",
      ".prov-policy-banner-reason",
      ".prov-policy-banner-remedy",
      ".prov-policy-banner-error",
    ]) {
      expect(proof.rulesFor(selector), selector).not.toBe("");
    }
  });

  it("grounds the banner on the house card with the failed-closed edge", () => {
    proof.inject([".prov-policy-banner"]);
    const banner = styledBox("prov-policy-banner");
    const style = getComputedStyle(banner);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    expect(style.backgroundColor).toBe(proof.token("--panel-card"));
    expect(proof.rulesFor(".prov-policy-banner")).toContain(proof.token("--danger"));
    expect(style.display).toBe("flex");
    expect(style.flexDirection).toBe("column");
  });

  it("sets the title at interface 14 semibold in the failed-closed colour", () => {
    proof.inject([".prov-policy-banner-title"]);
    const title = styledBox("prov-policy-banner-title");
    const style = getComputedStyle(title);
    expect(style.fontSize).toBe("14px");
    expect(style.fontWeight).toBe("600");
    expect(style.color).toBe(proof.token("--danger"));
  });

  it("lets a long reason wrap at small 13 without overflowing", () => {
    proof.inject([".prov-policy-banner-reason"]);
    const reason = styledBox("prov-policy-banner-reason");
    reason.textContent =
      "C:\\daemon\\tool-policies.json: Only one usage of each socket address (os error 10048)";
    const style = getComputedStyle(reason);
    expect(style.fontSize).toBe("13px");
    expect(style.color).toBe(proof.token("--ink-soft"));
    expect(proof.rulesFor(".prov-policy-banner-reason")).toMatch(
      /overflow-wrap:\s*(anywhere|break-word)/,
    );
  });

  it("keeps the remedy and the error at meta 12 in their roles", () => {
    proof.inject([".prov-policy-banner-remedy", ".prov-policy-banner-error"]);
    const remedy = styledBox("prov-policy-banner-remedy");
    expect(getComputedStyle(remedy).fontSize).toBe("12px");
    expect(getComputedStyle(remedy).color).toBe(proof.token("--muted"));
    const error = styledBox("prov-policy-banner-error");
    expect(getComputedStyle(error).fontSize).toBe("12px");
    expect(getComputedStyle(error).color).toBe(proof.token("--danger"));
    expect(proof.rulesFor(".prov-policy-banner-error")).toMatch(
      /overflow-wrap:\s*(anywhere|break-word)/,
    );
  });

  it("reuses the settings text button at 28 with its disabled look", async () => {
    proof.inject([
      ".prov-policy-banner",
      ".settings-device-action",
      ".settings-device-action:disabled",
    ]);
    const holder = document.createElement("div");
    holder.className = "prov-policy-banner";
    document.body.appendChild(holder);
    const idle = document.createElement("button");
    idle.className = "settings-device-action";
    idle.textContent = "Restart daemon";
    holder.appendChild(idle);
    expect(getComputedStyle(idle).height).toBe("28px");
    expect(getComputedStyle(idle).fontSize).toBe("14px");
    const busy = document.createElement("button");
    busy.className = "settings-device-action";
    busy.disabled = true;
    busy.textContent = "Restarting\u2026";
    holder.appendChild(busy);
    expect(getComputedStyle(busy).height).toBe("28px");
    expect(getComputedStyle(busy).opacity).toBe("0.55");
    expect(read("src/features/settings/providers.css")).not.toMatch(
      /\.prov-policy-banner[^}]*height:\s*28px/,
    );
  });
});
