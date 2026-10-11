// @vitest-environment happy-dom

// The sidebar row's archive: what the menu offers, the ask and its two answers,
// the mapped refusal, and where focus lands afterwards. Archive takes the row
// off the list and keeps its folder, whatever its isolation.
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  clickDialogButton,
  clickMenuEntry,
  dialog,
  DIALOG_SELECTOR,
  menuLabels,
  renderWorkspace,
  workspace as localWorkspace,
} from "./bulkCloseHarness";
import { workspaceArchive, workspacesList } from "../../lib/tauri";
import type { Workspace } from "../../types/ipc";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const worktreeRow: Workspace = { ...localWorkspace, isolation: "worktree" };

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

function rows(): HTMLButtonElement[] {
  return [...document.querySelectorAll<HTMLButtonElement>("button.workspace-row")];
}

async function openRowMenu(index = 0): Promise<void> {
  const row = rows()[index];
  if (row === undefined) throw new Error(`workspace row ${index} did not render`);
  await act(async () => {
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  });
}

async function askForArchive(index = 0): Promise<void> {
  await openRowMenu(index);
  await clickMenuEntry("Archive workspace");
}

function confirmButton(): HTMLButtonElement {
  const button = [...dialog().querySelectorAll<HTMLButtonElement>("button")].find(
    (candidate) => candidate.textContent === "Archive",
  );
  if (button === undefined) throw new Error("the ask's Archive button did not render");
  return button;
}

async function settle(): Promise<void> {
  await act(async () => undefined);
  await act(async () => undefined);
}

describe("archiving a workspace from the sidebar", () => {
  it("offers archive on a local row too: the folder is kept, so nothing is refused", async () => {
    await renderWorkspace();
    await openRowMenu();

    expect(menuLabels()).toEqual(["Rename", "Archive workspace"]);
  });

  it("offers archive on a worktree row as well", async () => {
    vi.mocked(workspacesList).mockResolvedValue([worktreeRow]);
    await renderWorkspace();
    await openRowMenu();

    expect(menuLabels()).toEqual(["Rename", "Archive workspace", "Delete worktree"]);
  });

  it("the row's kebab opens the same menu a right click does", async () => {
    await renderWorkspace();
    const kebab = document.querySelector<HTMLButtonElement>(".workspace-row-kebab");
    if (kebab === null) throw new Error("the row's kebab did not render");
    expect(kebab.getAttribute("aria-label")).toBe(`Actions for ${localWorkspace.title}`);

    await act(async () => kebab.click());

    expect(menuLabels()).toEqual(["Rename", "Archive workspace"]);
  });

  it("asks first with Cancel focused, then calls the archive once and keeps the folder in the words", async () => {
    await renderWorkspace();
    await askForArchive();

    const ask = dialog();
    expect(ask.textContent).toContain("Takes this workspace off the list");
    expect(ask.textContent).toContain("stay on disk");
    const cancel = [...ask.querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "Cancel",
    );
    expect(cancel, "the ask's safe answer").toBeDefined();
    expect(document.activeElement, "the safe answer takes focus on open").toBe(cancel);

    await clickDialogButton("Archive");

    expect(workspaceArchive).toHaveBeenCalledTimes(1);
    expect(workspaceArchive).toHaveBeenCalledWith("workspace-1");
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
  });

  it("Cancel calls nothing and leaves the row", async () => {
    await renderWorkspace();
    await askForArchive();
    await clickDialogButton("Cancel");

    expect(workspaceArchive).not.toHaveBeenCalled();
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
    expect(document.querySelector("button.workspace-row")).not.toBeNull();
  });

  it("two confirms in the same tick issue one archive", async () => {
    await renderWorkspace();
    await askForArchive();

    const confirm = confirmButton();
    await act(async () => {
      confirm.click();
      confirm.click();
    });
    await settle();

    expect(workspaceArchive).toHaveBeenCalledTimes(1);
  });

  it("shows the live-session refusal as its own sentence, never the raw text", async () => {
    vi.mocked(workspaceArchive).mockRejectedValueOnce({
      code: "invalid_request",
      message: "sessions or terminals are still running in this workspace; close them first",
    });
    await renderWorkspace();
    await askForArchive();
    await clickDialogButton("Archive");
    await settle();

    expect(document.body.textContent).toContain(
      "Stop the agents and terminals in this workspace first.",
    );
    expect(document.body.innerHTML).not.toContain("sessions or terminals are still running");
    expect(
      document.querySelector("button.workspace-row"),
      "a refusal must keep the row, never drop it optimistically",
    ).not.toBeNull();
    expect(document.querySelector(DIALOG_SELECTOR)).toBeNull();
  });

  it("after a success the row leaves through the re-read, the tabs go empty, focus lands on the project's create control", async () => {
    await renderWorkspace();
    vi.mocked(workspacesList).mockResolvedValue([]);
    await askForArchive();
    await clickDialogButton("Archive");
    await settle();

    expect(workspaceArchive).toHaveBeenCalledTimes(1);
    expect(document.querySelector("button.workspace-row")).toBeNull();
    expect(document.body.textContent).toContain("No tabs yet");
    expect(document.activeElement).toBe(document.querySelector(".workspace-project-new"));
    expect(document.activeElement).not.toBe(document.body);
  });

  it("after a success focus moves to the next workspace row", async () => {
    vi.mocked(workspacesList).mockResolvedValue([
      worktreeRow,
      { ...localWorkspace, id: "workspace-2", title: "second" },
    ]);
    await renderWorkspace();
    const survivor = rows()[1];
    if (survivor === undefined) throw new Error("second workspace row did not render");
    vi.mocked(workspacesList).mockResolvedValue([
      { ...localWorkspace, id: "workspace-2", title: "second" },
    ]);
    await askForArchive();
    await clickDialogButton("Archive");
    await settle();

    expect(document.activeElement).toBe(survivor);
    expect(document.querySelectorAll("button.workspace-row")).toHaveLength(1);
  });

  it("when a search leaves only the archived row, focus falls to the search field, not the body", async () => {
    const found = { ...worktreeRow, title: "feature-x" };
    vi.mocked(workspacesList).mockResolvedValue([
      found,
      { ...localWorkspace, id: "workspace-2", title: "second" },
    ]);
    await renderWorkspace();
    const trigger = document.querySelector<HTMLButtonElement>(".sidebar-search-trigger");
    if (trigger === null) throw new Error("the sidebar search row did not render");
    await act(async () => trigger.click());
    const search = document.querySelector<HTMLInputElement>(".workspace-search input");
    if (search === null) throw new Error("search input did not render");
    await act(async () => {
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      setter?.call(search, "feature-x");
      search.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(rows(), "the search keeps only the row about to go").toHaveLength(1);
    vi.mocked(workspacesList).mockResolvedValue([
      { ...localWorkspace, id: "workspace-2", title: "second" },
    ]);
    await askForArchive();
    await clickDialogButton("Archive");
    await settle();

    expect(document.querySelector(".workspace-project-new"), "the whole group left").toBeNull();
    expect(document.activeElement).toBe(search);
  });
});
