// @vitest-environment happy-dom

// The row's "Delete worktree": offered on a local worktree row only, after
// Archive. It asks first, and a refusal is said in the row while the row stays.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import { LOCAL_HOST_ID, localWorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceView } from "../workspaceProjects";
import { WorkspaceRow } from "./WorkspaceRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function view(over: Partial<WorkspaceView> = {}): WorkspaceView {
  return {
    id: "workspace-1",
    projectId: "project-1",
    hostId: LOCAL_HOST_ID,
    title: "feat-cart",
    displayTitle: "feat-cart",
    isolation: "worktree",
    path: "C:\\dev\\feat-cart",
    agents: { working: 0, waiting: 0 },
    stateDot: null,
    ...over,
  };
}

describe("the Delete worktree entry of the row menu", () => {
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

  async function render(
    over: Partial<WorkspaceView>,
    onDeleteWorktree: (workspaceId: string) => Promise<ErrorSentence | null> = vi.fn(
      async () => null,
    ),
  ): Promise<void> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspaceRow
          workspace={view(over)}
          workspaceKey={localWorkspaceKey("workspace-1")}
          hostName="This PC"
          selected={false}
          stat={undefined}
          branch={undefined}
          onSelect={vi.fn()}
          onRename={vi.fn(async () => null)}
          onArchive={vi.fn(async () => null)}
          onDeleteWorktree={onDeleteWorktree}
        />,
      );
    });
  }

  function menuLabels(): string[] {
    return [...container.querySelectorAll('.workspace-row-menu [role="menuitem"]')].map(
      (item) => item.textContent ?? "",
    );
  }

  function menuEntry(label: string): HTMLButtonElement | undefined {
    return [
      ...container.querySelectorAll<HTMLButtonElement>('.workspace-row-menu [role="menuitem"]'),
    ].find((item) => item.textContent === label);
  }

  async function openMenu(): Promise<void> {
    const kebab = container.querySelector<HTMLButtonElement>(".workspace-row-kebab");
    await act(async () => kebab?.click());
  }

  /** The dialog's own confirm button: the menu entry has the same words, so the menu is skipped. */
  function dialogConfirm(): HTMLButtonElement | undefined {
    return [...document.body.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) =>
        button.textContent === "Delete worktree" && button.getAttribute("role") !== "menuitem",
    );
  }

  it("offers Delete worktree after Archive, on a worktree row", async () => {
    await render({});
    await openMenu();

    expect(menuLabels()).toEqual(["Rename", "Archive workspace", "Delete worktree"]);
  });

  it("offers no Delete worktree on a local row: that is the project folder", async () => {
    await render({ isolation: "local" });
    await openMenu();

    expect(menuLabels()).not.toContain("Delete worktree");
  });

  it("asks first, and removes the worktree on its own row only when confirmed", async () => {
    const onDelete = vi.fn(async () => null);
    await render({}, onDelete);
    await openMenu();
    const entry = menuEntry("Delete worktree");
    await act(async () => entry?.click());

    expect(onDelete).not.toHaveBeenCalled();
    await act(async () => dialogConfirm()?.click());

    expect(onDelete).toHaveBeenCalledTimes(1);
    expect(onDelete).toHaveBeenCalledWith("workspace-1");
  });

  it("says a refusal in the row, and the row stays on the list", async () => {
    const refusal = errorSentence(
      new Error("sessions or terminals are still running in this workspace; close them first"),
    );
    const onDelete = vi.fn(async () => refusal);
    await render({}, onDelete);
    await openMenu();
    const entry = menuEntry("Delete worktree");
    await act(async () => entry?.click());
    await act(async () => dialogConfirm()?.click());
    await act(async () => undefined);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(refusal.sentence);
    expect(container.querySelector(".workspace-row")).not.toBeNull();
  });
});
