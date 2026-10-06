// @vitest-environment happy-dom

// The foot's contract with the two ways of reading it: only the dot is drawn,
// what it stands for rides in the tooltip a pointer reads and in the status
// text a screen reader reads, and nothing here is a tab stop.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DaemonStatus } from "../../../types/ipc";
import { SidebarFooter } from "./SidebarFooter";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const CONNECTED: DaemonStatus = {
  state: "connected",
  pid: 40220,
  instanceId: "daemon-test",
  protocolVersion: 21,
  clients: 1,
  capabilities: [],
  message: null,
};

describe("the sidebar foot's status dot", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(daemon: DaemonStatus, note: string | null = null): Promise<HTMLElement> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <SidebarFooter historyOpen={false} onToggleHistory={vi.fn()} daemon={daemon} note={note} />,
      );
    });
    const foot = container.querySelector<HTMLElement>(".sidebar-foot");
    if (foot === null) throw new Error("the foot did not render");
    return foot;
  }

  it("draws the dot and no printed word beside it", async () => {
    const foot = await render(CONNECTED);

    expect(foot.querySelector(".workspace-status-dot")?.className).toContain("workspace-dot-green");
    expect(foot.querySelector(".workspace-daemon-status-label")).toBeNull();
    expect(foot.textContent).not.toContain("Daemon");
  });

  it("gives a pointer and a screen reader the same sentence, pid included", async () => {
    const foot = await render(CONNECTED, "restart failed");

    expect(foot.getAttribute("title")).toBe("daemon · pid 40220 · restart failed");
    expect(foot.querySelector(".sr-only")?.textContent).toBe("daemon · pid 40220 · restart failed");
  });

  it("announces the state it holds and takes no tab stop of its own", async () => {
    const foot = await render({
      ...CONNECTED,
      state: "disconnected",
      message: "daemon unreachable",
    });

    expect(foot.getAttribute("tabindex")).toBeNull();
    expect(foot.getAttribute("role")).toBe("status");
    expect(foot.querySelector(".workspace-dot-terracotta")).not.toBeNull();
    expect(foot.textContent).toContain("daemon · ");
  });
});
