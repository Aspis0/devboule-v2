// @vitest-environment happy-dom

// The sidebar's footer is the Settings button alone: the daemon's status is
// shown once, in the status bar, so the footer draws no dot.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarFooter } from "./SidebarFooter";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("the sidebar footer", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(onOpenSettings = vi.fn()): Promise<HTMLElement> {
    await act(async () => {
      root.render(<SidebarFooter onOpenSettings={onOpenSettings} />);
    });
    const row = container.querySelector<HTMLElement>('.sidebar-icon-row[role="group"]');
    if (row === null) throw new Error("the icon row did not render");
    return row;
  }

  it("holds the Settings button and no project button", async () => {
    const onOpenSettings = vi.fn();
    const row = await render(onOpenSettings);

    expect(row.querySelector('button[aria-label="New project"]')).toBeNull();
    const settings = row.querySelector<HTMLButtonElement>('button[aria-label="Settings"]');
    if (!settings) throw new Error("the Settings button did not render");
    await act(async () => settings.click());
    expect(onOpenSettings).toHaveBeenCalledTimes(1);
    expect(row.querySelectorAll("button")).toHaveLength(1);
  });

  it("draws no daemon dot, so the status is not repeated here", async () => {
    await render();

    expect(container.querySelector(".workspace-status-dot")).toBeNull();
    expect(container.querySelector(".sidebar-foot")).toBeNull();
  });
});
