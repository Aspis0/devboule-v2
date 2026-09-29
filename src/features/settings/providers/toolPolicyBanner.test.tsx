// @vitest-environment happy-dom

// The failed-closed banner: it shows the daemon's reason verbatim, never a
// rewritten claim, with the one remedy and a restart button behind a confirm.
import { ask } from "@tauri-apps/plugin-dialog";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { daemonRestart } from "../../../lib/tauri";
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
