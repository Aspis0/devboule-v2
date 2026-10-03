// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkspaceTree } from "./WorkspaceTree";
import type { WorkspaceProject } from "../workspaceProjects";
import type { ErrorSentence } from "../../../lib/errorSentence";
import { LOCAL_HOST_ID, localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";

/** The workspace as the UI names it, for a fixture that only knows the daemon id. */
const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const project: WorkspaceProject = {
  id: "project-1",
  name: "devboule-v2",
  hostId: LOCAL_HOST_ID,
  path: "C:\\devboule-v2",
  workspaces: [
    {
      id: "workspace-1",
      projectId: "project-1",
      title: "devboule-v2",
      displayTitle: "devboule-v2",
      hostId: LOCAL_HOST_ID,
      isolation: "local",
      path: "C:\\devboule-v2",
      meta: null,
      stateDot: null,
    },
    {
      id: "workspace-2",
      projectId: "project-1",
      title: "devboule-v2",
      displayTitle: "devboule-v2 2",
      hostId: LOCAL_HOST_ID,
      isolation: "local",
      path: "C:\\devboule-v2",
      meta: null,
      stateDot: null,
    },
  ],
};

describe("the sidebar's workspace rows", () => {
  let container: HTMLDivElement;
  let root: Root;
  const onRename = vi.fn(
    async (_workspaceId: string, _title: string): Promise<ErrorSentence | null> => null,
  );
  const onDeleteWorkspace = vi.fn(
    async (_workspaceId: string): Promise<ErrorSentence | null> => null,
  );

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    onRename.mockClear();
    onRename.mockResolvedValue(null);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.clearAllMocks();
  });

  async function renderTree() {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspaceTree
          projects={[project]}
          loading={false}
          error={null}
          providerError={null}
          selectedWorkspace={keyFor("workspace-1")}
          onRetryProjects={vi.fn()}
          onSelectWorkspace={vi.fn()}
          onNewWorkspace={vi.fn()}
          providerMenuAnchorProjectId={null}
          providerMenu={null}
          stats={new Map()}
          onRenameWorkspace={onRename}
          onDeleteWorkspace={onDeleteWorkspace}
        />,
      );
    });
  }

  function rowButtons(): HTMLButtonElement[] {
    return [...container.querySelectorAll<HTMLButtonElement>(".workspace-row")];
  }

  async function openContextMenu(index: number) {
    const row = rowButtons()[index];
    if (row === undefined) throw new Error(`workspace row ${index} did not render`);
    await act(async () => {
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });
  }

  function menuItem(label: string): HTMLButtonElement | null {
    return (
      [...container.querySelectorAll<HTMLButtonElement>("[role='menuitem']")].find(
        (button) => button.textContent === label,
      ) ?? null
    );
  }

  function editInput(): HTMLInputElement | null {
    return container.querySelector<HTMLInputElement>("input[aria-label='Workspace title']");
  }

  async function type(input: HTMLInputElement, value: string) {
    const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    if (setValue === undefined) throw new Error("input value setter did not exist");
    await act(async () => {
      setValue.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  async function press(input: HTMLInputElement, key: string, options?: { composing?: boolean }) {
    await act(async () => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", {
          key,
          bubbles: true,
          isComposing: options?.composing ?? false,
        }),
      );
    });
  }

  function deferSave(): (error: ErrorSentence | null) => void {
    let release: ((error: ErrorSentence | null) => void) | undefined;
    onRename.mockReturnValueOnce(
      new Promise<ErrorSentence | null>((resolve) => {
        release = resolve;
      }),
    );
    return (error) => release!(error);
  }

  async function openEditor(index: number, draft?: string): Promise<HTMLInputElement> {
    await openContextMenu(index);
    const rename = menuItem("Rename");
    expect(rename, "Rename did not render in the row's context menu").not.toBeNull();
    await act(async () => rename!.click());
    const input = editInput();
    expect(input, "the row's inline title editor did not open").not.toBeNull();
    if (draft !== undefined) await type(input!, draft);
    return input!;
  }

  async function blurEditor() {
    const input = editInput();
    expect(input, "the editor must be open to leave it").not.toBeNull();
    await act(async () => input!.blur());
  }

  it("reads two rows that share a title apart, numbering by creation order", async () => {
    await renderTree();

    const titles = rowButtons().map(
      (row) => row.querySelector(".workspace-row-title")?.textContent,
    );
    expect(titles).toEqual(["devboule-v2", "devboule-v2 2"]);
  });

  it("opens Rename from the row's context menu and saves with Enter", async () => {
    await renderTree();
    const input = await openEditor(1, "devboule-v2 beta");
    await press(input, "Enter");

    expect(onRename).toHaveBeenCalledTimes(1);
    expect(onRename).toHaveBeenCalledWith("workspace-2", "devboule-v2 beta");
    expect(editInput()).toBeNull();
  });

  it("cancels the edit with Escape, calling nothing", async () => {
    await renderTree();
    const input = await openEditor(1, "half typed");
    await press(input, "Escape");

    expect(onRename).not.toHaveBeenCalled();
    expect(editInput()).toBeNull();
    expect(rowButtons()[1]?.querySelector(".workspace-row-title")?.textContent).toBe(
      "devboule-v2 2",
    );
  });

  it("leaves an IME's Enter to the composition instead of saving", async () => {
    await renderTree();
    const input = await openEditor(1, "devboule-v2 beta");
    await press(input, "Enter", { composing: true });

    expect(onRename).not.toHaveBeenCalled();
    expect(editInput(), "a composing Enter is the IME's commit, not the row's save").not.toBeNull();
  });

  it("leaves an IME's Escape to the composition instead of closing", async () => {
    await renderTree();
    const input = await openEditor(1, "half typed");
    await press(input, "Escape", { composing: true });

    expect(
      editInput(),
      "a composing Escape is the IME's cancel, not the row's close",
    ).not.toBeNull();
  });

  it("refuses an empty title and keeps the editor open", async () => {
    await renderTree();
    const input = await openEditor(0, "   ");
    await press(input, "Enter");

    expect(onRename).not.toHaveBeenCalled();
    expect(editInput(), "an empty title must not close the editor").not.toBeNull();
    expect(container.textContent).toContain("A workspace title is required; it was empty.");
  });

  it("hands focus back to the row once Enter has saved", async () => {
    await renderTree();
    const input = await openEditor(1, "devboule-v2 beta");
    await press(input, "Enter");

    expect(editInput()).toBeNull();
    expect(document.activeElement).toBe(rowButtons()[1]);
  });

  it("hands focus back to the row once Escape has cancelled", async () => {
    await renderTree();
    const input = await openEditor(1, "half typed");
    await press(input, "Escape");

    expect(editInput()).toBeNull();
    expect(document.activeElement).toBe(rowButtons()[1]);
  });

  it("leaves focus on the control the person clicked when an in-flight save resolves", async () => {
    const release = deferSave();
    await renderTree();
    const input = await openEditor(1, "devboule-v2 beta");
    await press(input, "Enter");
    expect(editInput(), "the save is still in flight").not.toBeNull();

    const other = document.createElement("button");
    container.appendChild(other);
    await act(async () => {
      input.blur();
      other.focus();
    });

    release(null);
    await act(async () => undefined);

    expect(editInput()).toBeNull();
    expect(document.activeElement).toBe(other);
    other.remove();
  });

  it("locks the input while a save is in flight and closes only when it settles", async () => {
    const release = deferSave();
    await renderTree();
    const input = await openEditor(1, "devboule-v2 beta");
    await press(input, "Enter");

    expect(input.readOnly, "the draft on its way must not change").toBe(true);
    expect(input.getAttribute("aria-busy")).toBe("true");
    await press(input, "Escape");
    expect(editInput(), "Escape must wait for the save").not.toBeNull();

    release(null);
    await act(async () => undefined);

    expect(editInput()).toBeNull();
  });

  it("hands the input back editable when the save is refused", async () => {
    const release = deferSave();
    await renderTree();
    const input = await openEditor(1, "devboule-v2 beta");
    await press(input, "Enter");
    expect(input.readOnly).toBe(true);

    release({
      sentence: "The agent daemon refused that request as invalid.",
      detail: "raw words",
    });
    await act(async () => undefined);

    const open = editInput();
    expect(open, "a refusal keeps the editor open").not.toBeNull();
    expect(open!.readOnly, "a refusal hands the draft back for editing").toBe(false);
    expect(container.textContent).toContain("The agent daemon refused that request as invalid.");
  });

  it("commits a changed and valid draft when focus leaves the input", async () => {
    await renderTree();
    await openEditor(1, "devboule-v2 beta");
    await blurEditor();

    expect(onRename).toHaveBeenCalledWith("workspace-2", "devboule-v2 beta");
    expect(editInput()).toBeNull();
  });

  it("cancels an unchanged draft when focus leaves the input", async () => {
    await renderTree();
    await openEditor(1);
    await blurEditor();

    expect(onRename).not.toHaveBeenCalled();
    expect(editInput()).toBeNull();
    expect(rowButtons()[1]?.querySelector(".workspace-row-title")?.textContent).toBe(
      "devboule-v2 2",
    );
  });

  it("cancels an empty draft when focus leaves the input", async () => {
    await renderTree();
    await openEditor(1, "   ");
    await blurEditor();

    expect(onRename).not.toHaveBeenCalled();
    expect(editInput()).toBeNull();
  });

  it("keeps the editor open with the plain sentence when a blur's save is refused", async () => {
    onRename.mockResolvedValue({
      sentence: "The agent daemon refused that request as invalid.",
      detail: "internal words",
    });
    await renderTree();
    await openEditor(1, "devboule-v2 beta");
    await blurEditor();

    expect(editInput(), "a refused save keeps the editor open").not.toBeNull();
    expect(container.textContent).toContain("The agent daemon refused that request as invalid.");
  });
});
