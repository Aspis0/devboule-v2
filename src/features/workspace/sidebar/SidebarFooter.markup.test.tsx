// @vitest-environment happy-dom

// The foot's contract as a bottom icon row: add project and settings name
// themselves, and the daemon dot keeps its contract with the three ways of
// reading it — only the dot is drawn at rest, what it stands for rides in the
// tooltip a pointer reads, in the status text a screen reader reads and in the
// tip keyboard focus reveals, and the status line is exactly one tab stop.

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
        <SidebarFooter
          onAddProject={vi.fn()}
          addProjectRef={{ current: null }}
          onOpenSettings={vi.fn()}
          daemon={daemon}
          note={note}
        />,
      );
    });
    const foot = container.querySelector<HTMLElement>(".workspace-sidebar-footer");
    if (foot === null) throw new Error("the foot did not render");
    return foot;
  }

  function dot(): HTMLElement {
    const status = container.querySelector<HTMLElement>(".sidebar-foot");
    if (status === null) throw new Error("the daemon status did not render");
    return status;
  }

  it("is an icon row: New project, Settings, and the daemon dot", async () => {
    const onAddProject = vi.fn();
    const onOpenSettings = vi.fn();
    root = createRoot(container);
    await act(async () => {
      root.render(
        <SidebarFooter
          onAddProject={onAddProject}
          addProjectRef={{ current: null }}
          onOpenSettings={onOpenSettings}
          daemon={CONNECTED}
          note={null}
        />,
      );
    });

    const row = container.querySelector<HTMLElement>('.sidebar-icon-row[role="toolbar"]');
    if (row === null) throw new Error("the icon row did not render");
    const add = row.querySelector<HTMLButtonElement>('button[aria-label="New project"]');
    const settings = row.querySelector<HTMLButtonElement>('button[aria-label="Settings"]');
    if (!add || !settings) throw new Error("the icon row's buttons did not render");
    await act(async () => add.click());
    await act(async () => settings.click());
    expect(onAddProject).toHaveBeenCalledTimes(1);
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
    expect(row.querySelector(".sidebar-foot")).not.toBeNull();
    // The old bottom-of-sidebar History is gone from the foot.
    expect(container.querySelector(".workspace-history-button")).toBeNull();
  });

  it("draws the dot and no printed word beside it", async () => {
    await render(CONNECTED);
    const foot = dot();

    expect(foot.querySelector(".workspace-status-dot")?.className).toContain("workspace-dot-green");
    expect(foot.querySelector(".workspace-daemon-status-label")).toBeNull();
    expect(foot.textContent).not.toContain("Daemon");
  });

  it("gives a pointer and a screen reader the same sentence, pid included", async () => {
    await render(CONNECTED, "restart failed");
    const foot = dot();

    expect(foot.getAttribute("title")).toBe("daemon · pid 40220 · restart failed");
    expect(foot.querySelector(".sr-only")?.textContent).toBe("daemon · pid 40220 · restart failed");
  });

  it("announces the state it holds and is one tab stop whose tip a screen reader skips", async () => {
    await render({
      ...CONNECTED,
      state: "disconnected",
      message: "daemon unreachable",
    });
    const foot = dot();

    expect(foot.getAttribute("tabindex")).toBe("0");
    expect(foot.querySelectorAll("[tabindex]")).toHaveLength(0);
    const tip = foot.querySelector(".sidebar-foot-tip");
    expect(tip?.getAttribute("aria-hidden")).toBe("true");
    expect(tip?.textContent).toBe(foot.querySelector(".sr-only")?.textContent);
    expect(foot.getAttribute("role")).toBe("status");
    expect(foot.querySelector(".workspace-dot-terracotta")).not.toBeNull();
    expect(foot.textContent).toContain("daemon · ");
  });
});
