// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkspaceDirectory,
  WorkspaceFileEntry,
  WorkspaceFileMutation,
} from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFilesList: vi.fn(),
  workspaceFileRead: vi.fn(),
  workspaceFileRename: vi.fn(),
  workspaceFileDuplicate: vi.fn(),
  workspaceFileDelete: vi.fn(),
}));

// The confirmation belongs to the one act that loses data: the delete's gate
// lives inside `deleteEntry`, and these tests drive our dialog instead of
// a native mock — were the gate ever dropped from that road, the
// dialog-absence assertions below would die first. The two acts that lose
// no data must never raise it at all, which their own tests assert.
import { ConfirmProvider } from "../../components/ConfirmHost";
import { useAppStore } from "../../store/appStore";
import {
  workspaceFileDelete,
  workspaceFileDuplicate,
  workspaceFileRead,
  workspaceFileRename,
  workspaceFilesList,
} from "../../lib/tauri";
import { FilesSurface } from "./FilesSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-file-actions-subject";

function entry(
  path: string,
  kind: WorkspaceFileEntry["kind"],
  size: number | null = null,
): WorkspaceFileEntry {
  const segments = path.split("/");
  return { path, name: segments[segments.length - 1], kind, size };
}

function listing(
  entries: WorkspaceFileEntry[],
  overrides: Partial<WorkspaceDirectory> = {},
): WorkspaceDirectory {
  return { path: "", entries, capped: false, skipped: 0, error: null, ...overrides };
}

/** The root folder as the tests' stand-in disk holds it. */
let rootEntries: WorkspaceFileEntry[] = [];

describe("FilesFileActions", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    useAppStore.setState({ modalOpenTokens: new Set() });
    rootEntries = [entry("src", "dir"), entry("README.md", "file", 12)];
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) => {
      if (path === "") return Promise.resolve(listing(rootEntries));
      if (path === "src") return Promise.resolve(listing([entry("src/index.ts", "file", 6)]));
      if (path === "lib") return Promise.resolve(listing([entry("lib/index.ts", "file", 6)]));
      return Promise.resolve(listing([]));
    });
    vi.mocked(workspaceFileRead).mockResolvedValue({
      status: "ok",
      kind: "text",
      content: "",
      size: 0,
      modifiedAt: 0,
      error: null,
      fromLine: 1,
      lines: 0,
      hasMore: false,
      truncated: false,
      note: null,
    });
    vi.mocked(workspaceFileRename).mockResolvedValue({ newPath: "README.md", error: null });
    vi.mocked(workspaceFileDuplicate).mockResolvedValue({ newPath: "README copy.md", error: null });
    vi.mocked(workspaceFileDelete).mockResolvedValue({ newPath: null, error: null });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    useAppStore.setState({ modalOpenTokens: new Set() });
    vi.clearAllMocks();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(<ConfirmProvider>{ui}</ConfirmProvider>);
    });
  }

  /** The row menu's trigger of one entry, opened. */
  async function openMenu(path: string): Promise<void> {
    const name = pathName(path);
    const trigger = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-tree-menu-trigger"),
    ).find((button) => button.getAttribute("aria-label") === `${name} actions`);
    if (trigger === undefined) throw new Error(`no row rendered: ${path}`);
    await act(async () => {
      trigger.click();
    });
  }

  function pathName(path: string): string {
    const cut = path.lastIndexOf("/");
    return cut === -1 ? path : path.slice(cut + 1);
  }

  async function menuItem(label: string): Promise<void> {
    const item = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((button) => button.textContent === label);
    if (item === undefined) throw new Error(`menu item did not render: ${label}`);
    await act(async () => {
      item.click();
    });
  }

  /** The standing ask, portaled to the body — never inside the panel. */
  function confirmDialog(): HTMLElement {
    const found = document.querySelector<HTMLElement>(".confirm-dialog");
    if (found === null) throw new Error("confirm dialog did not render");
    return found;
  }

  function affirmative(): HTMLButtonElement {
    const found = document.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");
    if (found === null) throw new Error("confirm button did not render");
    return found;
  }

  function dialogBody(): string {
    return confirmDialog().querySelector(".confirm-dialog-body")?.textContent ?? "";
  }

  async function answerConfirm(): Promise<void> {
    await act(async () => {
      affirmative().click();
    });
  }

  async function answerCancel(): Promise<void> {
    const found = document.querySelector<HTMLButtonElement>(".confirm-dialog-cancel");
    if (found === null) throw new Error("cancel button did not render");
    await act(async () => {
      found.click();
    });
  }

  async function escapeAsk(): Promise<void> {
    await act(async () => {
      confirmDialog().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
  }

  async function scrimAsk(): Promise<void> {
    await act(async () => {
      document
        .querySelector<HTMLElement>(".confirm-dialog-backdrop")!
        .dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
  }

  function renameInput(): HTMLInputElement {
    const input = container.querySelector<HTMLInputElement>(".workspace-tree-rename");
    if (input === null) throw new Error("the inline rename input did not render");
    return input;
  }

  /** Type into a controlled input the way a user's keystrokes reach it. */
  async function typeName(value: string): Promise<void> {
    const input = renameInput();
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter on HTMLInputElement");
    await act(async () => {
      setter.call(input, value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
  }

  async function pressKey(key: string, init: KeyboardEventInit = {}): Promise<void> {
    await act(async () => {
      renameInput().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, ...init }));
    });
  }

  const alertText = (): string | null =>
    container.querySelector('[role="alert"]')?.textContent ?? null;

  it("renames a row inline: no confirmation, the wire called once, the parent re-read", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    expect(vi.mocked(workspaceFilesList).mock.calls).toEqual([[WORKSPACE, ""]]);

    await openMenu("README.md");
    await menuItem("Rename");
    expect(renameInput().value, "the input starts on the row's own name").toBe("README.md");

    await typeName("GUIDE.md");
    await pressKey("Enter");
    await act(async () => undefined);

    expect(vi.mocked(workspaceFileRename).mock.calls).toEqual([
      [WORKSPACE, "README.md", "GUIDE.md"],
    ]);
    // Rename asks for nothing: only delete raises the dialog.
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    // The parent folder is re-read through the same guarded reader — the
    // tree shows the act's result without a manual refresh.
    expect(vi.mocked(workspaceFilesList).mock.calls).toContainEqual([WORKSPACE, ""]);
    expect(vi.mocked(workspaceFilesList).mock.calls.length).toBeGreaterThan(1);
    expect(container.querySelector(".workspace-tree-rename")).toBeNull();
    expect(alertText()).toBeNull();
  });

  it("duplicates a row without confirmation and shows the copy once the parent is re-read", async () => {
    // The mock plays daemon AND disk: when the act lands, the copy exists,
    // so the re-read below has something to find.
    vi.mocked(workspaceFileDuplicate).mockImplementation(async () => {
      rootEntries = [...rootEntries, entry("README copy.md", "file", 12)];
      return { newPath: "README copy.md", error: null };
    });
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Duplicate");
    await act(async () => undefined);

    expect(vi.mocked(workspaceFileDuplicate).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    // Duplicate asks for nothing: only delete raises the dialog.
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(container.textContent).toContain("README copy.md");
    expect(alertText()).toBeNull();
  });

  // A refusal re-reads the parent TOO: `git mv` can die between its move and
  // its index (declared on `rename_on_disk`), this tree has no poll, and a
  // half state left on screen is exactly the false phrase the slice hunts.
  // Kills the mutation that refreshes only on success.
  it("shows a refusal's own sentence under the toolbar and refreshes the tree anyway", async () => {
    const refusal = "an entry with that new name already exists";
    vi.mocked(workspaceFileRename).mockResolvedValue({ newPath: null, error: refusal });
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const reads = vi.mocked(workspaceFilesList).mock.calls.length;

    await openMenu("README.md");
    await menuItem("Rename");
    await typeName("src");
    await pressKey("Enter");
    await act(async () => undefined);

    expect(alertText()).toBe(refusal);
    // The parent folder was asked again — one more read than the initial
    // one — while the tree keeps whatever answer comes back.
    expect(vi.mocked(workspaceFilesList).mock.calls.length).toBe(reads + 1);
    expect(vi.mocked(workspaceFilesList).mock.calls.at(-1)).toEqual([WORKSPACE, ""]);
    // No name moved, so no re-key happened: the input stays open under its
    // own sentence, to be corrected.
    expect(renameInput().value).toBe("src");
  });

  it("shows a transport failure the same way, without losing the edit", async () => {
    vi.mocked(workspaceFileRename).mockRejectedValue(new Error("the daemon did not answer"));
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Rename");
    await typeName("GUIDE.md");
    await pressKey("Enter");
    await act(async () => undefined);

    expect(alertText()).toBe("the daemon did not answer");
    expect(renameInput().value).toBe("GUIDE.md");
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  it("abandons the rename on Escape without touching the wire", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Rename");
    await typeName("GUIDE.md");
    await pressKey("Escape");
    await act(async () => undefined);

    expect(container.querySelector(".workspace-tree-rename")).toBeNull();
    expect(vi.mocked(workspaceFileRename)).not.toHaveBeenCalled();
  });

  // Enter and Escape during an IME composition belong to the candidate list:
  // neither may reach the rename, and the typed name stays untouched.
  it("leaves Enter and Escape to an open IME composition", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Rename");
    await typeName("GUIDE.md");

    await pressKey("Enter", { isComposing: true });
    expect(vi.mocked(workspaceFileRename)).not.toHaveBeenCalled();
    expect(renameInput().value).toBe("GUIDE.md");

    // Older engines report the composition commit as keyCode 229 alone.
    await pressKey("Enter", { keyCode: 229 });
    expect(vi.mocked(workspaceFileRename)).not.toHaveBeenCalled();
    expect(renameInput().value).toBe("GUIDE.md");

    await pressKey("Escape", { isComposing: true });
    await act(async () => undefined);
    expect(container.querySelector(".workspace-tree-rename")).not.toBeNull();
    expect(renameInput().value).toBe("GUIDE.md");

    // Composition closed: the next Enter renames, as it always has.
    await pressKey("Enter");
    await act(async () => undefined);
    expect(vi.mocked(workspaceFileRename)).toHaveBeenCalledWith(WORKSPACE, "README.md", "GUIDE.md");
  });

  // The tree must FOLLOW a renamed folder: its keys (row, expansion, child
  // cells) move to the new spelling, its children are re-read under it, and
  // the expanded state survives — a folder that collapsed back to a path
  // nothing addresses would kill this case.
  it("keeps a renamed folder's expanded children under the new spelling", async () => {
    vi.mocked(workspaceFileRename).mockImplementation(async (_workspaceId, path, name) => {
      rootEntries = rootEntries.map((item) =>
        item.path === path ? { ...item, path: name, name } : item,
      );
      return { newPath: name, error: null };
    });
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    const srcRow = container.querySelector<HTMLButtonElement>('.workspace-tree-dir[title="src"]');
    if (srcRow === null) throw new Error("the src row did not render");
    await act(async () => {
      srcRow.click();
    });
    expect(container.textContent).toContain("index.ts");

    await openMenu("src");
    await menuItem("Rename");
    await typeName("lib");
    await pressKey("Enter");
    await act(async () => undefined);

    expect(vi.mocked(workspaceFileRename).mock.calls).toEqual([[WORKSPACE, "src", "lib"]]);
    const calls = vi.mocked(workspaceFilesList).mock.calls;
    expect(calls).toContainEqual([WORKSPACE, ""]);
    expect(calls).toContainEqual([WORKSPACE, "lib"]);
    // The row is `lib` now and still expanded — the children re-read under
    // the new key, not left behind under `src`.
    const libRow = container.querySelector<HTMLButtonElement>('.workspace-tree-dir[title="lib"]');
    if (libRow === null) throw new Error("the renamed row did not render");
    expect(libRow.getAttribute("aria-expanded")).toBe("true");
    expect(container.textContent).toContain("index.ts");
    expect(container.querySelector('[title="src"]')).toBeNull();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  // The selection follows the entry it points at: renaming the selected
  // file re-reads it under its new spelling, so the preview never keeps
  // showing bytes under a name the tree no longer has.
  it("moves the preview's selection along with the renamed file", async () => {
    // The mock plays the disk again: the rename lands, and the re-reads
    // that follow find the file under its new spelling.
    vi.mocked(workspaceFileRename).mockImplementation(async (_workspaceId, _path, name) => {
      rootEntries = rootEntries.map((item) =>
        item.path === "README.md" ? { ...item, path: name, name } : item,
      );
      return { newPath: name, error: null };
    });
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    const fileRow = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-file[title="README.md"]',
    );
    if (fileRow === null) throw new Error("the file row did not render");
    await act(async () => {
      fileRow.click();
    });
    expect(vi.mocked(workspaceFileRead).mock.calls).toEqual([[WORKSPACE, "README.md"]]);

    await openMenu("README.md");
    await menuItem("Rename");
    await typeName("GUIDE.md");
    await pressKey("Enter");
    await act(async () => undefined);

    expect(vi.mocked(workspaceFileRead).mock.calls).toContainEqual([WORKSPACE, "GUIDE.md"]);
    const selected = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-file[aria-current="true"]',
    );
    expect(selected?.getAttribute("title")).toBe("GUIDE.md");
  });

  // The slice's own shape (plan-write §2.4): choosing Delete raises our
  // dialog, and while the ask stands unanswered no command has reached the
  // wire. A No stops EVERYTHING — no command, and the tree is not even
  // re-read, because nothing happened to re-read. Kills the mutation that
  // drops the confirmation gate from `deleteEntry`.
  it("asks through our dialog, and a No stops everything before the wire", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");

    const card = confirmDialog();
    expect(card.querySelector(".confirm-dialog-title")?.textContent).toBe("Delete “README.md”");
    expect(dialogBody()).toContain("README.md");
    expect(dialogBody()).toContain("This cannot be undone.");
    expect(affirmative().textContent).toBe("Delete");
    expect(confirmDialog().querySelector(".confirm-dialog-cancel")?.textContent).toBe("Keep it");
    expect(affirmative().classList.contains("confirm-dialog-confirm-danger")).toBe(true);
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();

    await answerCancel();
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceFilesList).mock.calls).toEqual([[WORKSPACE, ""]]);
    expect(container.textContent).toContain("README.md");
    expect(alertText()).toBeNull();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  // Escape and the scrim decline the same way: no wire, no re-read.
  it("Escape and the scrim decline the delete", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");
    await escapeAsk();
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceFilesList).mock.calls).toEqual([[WORKSPACE, ""]]);
    expect(document.querySelector(".confirm-dialog")).toBeNull();

    await openMenu("README.md");
    await menuItem("Delete");
    await scrimAsk();
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceFilesList).mock.calls).toEqual([[WORKSPACE, ""]]);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(container.textContent).toContain("README.md");
  });

  // Unmounting with the ask standing (a workspace or panel switch remounts
  // the host) declines it: no wire, and the modal token goes with the dialog.
  it("unmounting with the ask standing declines it and leaks no modal", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");
    expect(document.querySelector(".confirm-dialog")).not.toBeNull();
    await act(async () => {
      root.unmount();
    });

    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  // The ask borrows focus and hands it back: Cancel lands on the row's
  // menu trigger the menu opened from — the dialog stays mounted and
  // closes through its `open` prop, so its own trigger return runs.
  it("Cancel hands focus back to the row's menu trigger", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const trigger = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-menu-trigger[aria-label="README.md actions"]',
    );
    if (trigger === null) throw new Error("the row trigger did not render");
    await act(async () => {
      trigger.focus();
    });

    await openMenu("README.md");
    await menuItem("Delete");
    expect(document.activeElement).toBe(document.querySelector(".confirm-dialog-cancel"));

    await answerCancel();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(document.activeElement).toBe(trigger);
  });

  // A mouse press moves focus onto the menu item before the click reaches
  // it; the item dies with the menu, so without the trigger's pre-focus
  // the dialog would capture a gone element and strand focus on <body>.
  it("returns focus to the trigger even when the menu item held focus at the ask", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const trigger = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-menu-trigger[aria-label="README.md actions"]',
    );
    if (trigger === null) throw new Error("the row trigger did not render");
    await act(async () => {
      trigger.focus();
    });

    await openMenu("README.md");
    const item = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((button) => button.textContent === "Delete");
    if (item === undefined) throw new Error("delete menu item did not render");
    await act(async () => {
      item.focus();
    });
    await menuItem("Delete");

    await answerCancel();
    expect(document.activeElement).toBe(trigger);
  });

  // A landed delete takes its own row with it: the re-read drops the
  // entry, the trigger is gone, and focus parks on the panel — never body.
  it("a confirmed delete that takes its row parks focus on the panel", async () => {
    vi.mocked(workspaceFileDelete).mockImplementation(async () => {
      rootEntries = rootEntries.filter((item) => item.path !== "README.md");
      return { newPath: null, error: null };
    });
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const trigger = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-menu-trigger[aria-label="README.md actions"]',
    );
    if (trigger === null) throw new Error("the row trigger did not render");
    await act(async () => {
      trigger.focus();
    });

    await openMenu("README.md");
    await menuItem("Delete");
    await answerConfirm();
    await act(async () => undefined);

    expect(vi.mocked(workspaceFileDelete)).toHaveBeenCalledTimes(1);
    expect(container.textContent).not.toContain("README.md");
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    const panel = container.querySelector<HTMLElement>(".workspace-files");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(document.body);
  });

  // The real order: the wire answers on its own latency and the parent
  // re-read lands after it — two separate commits, not one. The arm must
  // survive the flip on the stale tree and settle the row-gone case onto
  // the panel once the re-read arrives.
  it("a slow confirmed delete that takes its row lands on the panel once the re-read lands", async () => {
    let wireDone = false;
    let releaseRoot: ((value: WorkspaceDirectory) => void) | null = null;
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) => {
      if (path === "src") return Promise.resolve(listing([entry("src/index.ts", "file", 6)]));
      if (!wireDone || path !== "") return Promise.resolve(listing(rootEntries));
      return new Promise<WorkspaceDirectory>((resolve) => {
        releaseRoot = resolve;
      });
    });
    let resolveWire: ((value: WorkspaceFileMutation) => void) | null = null;
    vi.mocked(workspaceFileDelete).mockImplementation(
      () =>
        new Promise<WorkspaceFileMutation>((resolve) => {
          resolveWire = resolve;
        }),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const trigger = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-menu-trigger[aria-label="README.md actions"]',
    );
    if (trigger === null) throw new Error("the row trigger did not render");
    await act(async () => {
      trigger.focus();
    });

    await openMenu("README.md");
    await menuItem("Delete");
    await answerConfirm();
    expect(vi.mocked(workspaceFileDelete)).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    // The dialog unregistered before the wire started: the park reads a
    // count without the ask in it.
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
    const panel = container.querySelector<HTMLElement>(".workspace-files");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);

    await act(async () => {
      wireDone = true;
      resolveWire!({ newPath: null, error: null });
    });
    await act(async () => undefined);
    // The flip lands on the stale tree first: the repair borrows the row's
    // own trigger back until the re-read arrives.
    expect(document.activeElement).toBe(trigger);

    await act(async () => {
      rootEntries = rootEntries.filter((item) => item.path !== "README.md");
      releaseRoot!(listing(rootEntries));
    });
    await act(async () => undefined);
    expect(container.textContent).not.toContain("README.md");
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(document.body);
  });

  // No ask at all: a refresh that takes the focused row still lands on
  // the panel — the rescue answers every rows change, not just an act's.
  it("a refresh that takes the focused row lands on the panel with no ask behind it", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const trigger = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-menu-trigger[aria-label="README.md actions"]',
    );
    if (trigger === null) throw new Error("the row trigger did not render");
    await act(async () => {
      trigger.focus();
    });

    rootEntries = rootEntries.filter((item) => item.path !== "README.md");
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label="Refresh"]')!
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => undefined);
    expect(container.textContent).not.toContain("README.md");
    const panel = container.querySelector<HTMLElement>(".workspace-files");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(document.body);
  });

  // The rescue answers only for a focus the removal stranded: a row that
  // leaves while focus sits on an outside control moves nothing.
  it("moves no focus when the row leaves while focus is outside the panel", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const elsewhere = document.createElement("button");
    elsewhere.textContent = "elsewhere";
    document.body.appendChild(elsewhere);
    await act(async () => {
      elsewhere.focus();
    });

    rootEntries = rootEntries.filter((item) => item.path !== "README.md");
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label="Refresh"]')!
        .dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await act(async () => undefined);
    expect(container.textContent).not.toContain("README.md");
    expect(document.activeElement).toBe(elsewhere);
    elsewhere.remove();
  });

  // A nested delete re-reads its own parent folder, never the root: the
  // landing must answer to the whole cell map, not to the root's entries.
  it("a slow confirmed delete of a nested file lands on the panel once its folder is re-read", async () => {
    let wireDone = false;
    let releaseDeep: ((value: WorkspaceDirectory) => void) | null = null;
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) => {
      if (path === "") return Promise.resolve(listing(rootEntries));
      if (path === "src")
        return Promise.resolve(
          listing([entry("src/index.ts", "file", 6), entry("src/deep", "dir")]),
        );
      if (path === "src/deep") {
        if (!wireDone) return Promise.resolve(listing([entry("src/deep/file.ts", "file", 4)]));
        return new Promise<WorkspaceDirectory>((resolve) => {
          releaseDeep = resolve;
        });
      }
      return Promise.resolve(listing([]));
    });
    let resolveWire: ((value: WorkspaceFileMutation) => void) | null = null;
    vi.mocked(workspaceFileDelete).mockImplementation(
      () =>
        new Promise<WorkspaceFileMutation>((resolve) => {
          resolveWire = resolve;
        }),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    await act(async () => {
      container.querySelector<HTMLButtonElement>('.workspace-tree-dir[title="src"]')!.click();
    });
    await act(async () => {
      container.querySelector<HTMLButtonElement>('.workspace-tree-dir[title="src/deep"]')!.click();
    });
    const trigger = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-menu-trigger[aria-label="file.ts actions"]',
    );
    if (trigger === null) throw new Error("the nested row trigger did not render");
    await act(async () => {
      trigger.focus();
    });

    await openMenu("src/deep/file.ts");
    await menuItem("Delete");
    expect(dialogBody()).toContain("src/deep/file.ts");
    await answerConfirm();
    expect(vi.mocked(workspaceFileDelete)).toHaveBeenCalledWith(WORKSPACE, "src/deep/file.ts");
    const panel = container.querySelector<HTMLElement>(".workspace-files");
    expect(panel).not.toBeNull();
    expect(document.activeElement).toBe(panel);

    await act(async () => {
      wireDone = true;
      resolveWire!({ newPath: null, error: null });
    });
    await act(async () => undefined);
    expect(document.activeElement).toBe(trigger);

    await act(async () => {
      releaseDeep!(listing([]));
    });
    await act(async () => undefined);
    expect(container.querySelector('[title="src/deep/file.ts"]')).toBeNull();
    expect(document.activeElement).toBe(panel);
    expect(document.activeElement).not.toBe(document.body);
  });

  it("deletes once confirmed: the parent is re-read and a dead selection drops its preview", async () => {
    // The mock plays daemon AND disk: the entry is gone, so the parent
    // re-read below must not find it any more.
    vi.mocked(workspaceFileDelete).mockImplementation(async () => {
      rootEntries = rootEntries.filter((item) => item.path !== "README.md");
      return { newPath: null, error: null };
    });
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const fileRow = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-file[title="README.md"]',
    );
    if (fileRow === null) throw new Error("the file row did not render");
    await act(async () => {
      fileRow.click();
    });
    expect(vi.mocked(workspaceFileRead).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    expect(container.querySelector(".workspace-diff-card")).not.toBeNull();

    await openMenu("README.md");
    await menuItem("Delete");
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    await answerConfirm();

    expect(vi.mocked(workspaceFileDelete).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    expect(container.textContent).not.toContain("README.md");
    // The preview died with the file: no card still showing bytes of it, no
    // row left current, and no re-read of the dead path ever issued.
    expect(container.querySelector(".workspace-diff-card")).toBeNull();
    expect(container.querySelector('[aria-current="true"]')).toBeNull();
    expect(vi.mocked(workspaceFileRead).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    expect(alertText()).toBeNull();
  });

  it("deleting a folder takes the selection under it and names the folder in the confirmation", async () => {
    vi.mocked(workspaceFileDelete).mockImplementation(async (_workspaceId, path) => {
      rootEntries = rootEntries.filter((item) => item.path !== path);
      rootEntries = rootEntries.filter((item) => !item.path.startsWith(`${path}/`));
      return { newPath: null, error: null };
    });
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const srcRow = container.querySelector<HTMLButtonElement>('.workspace-tree-dir[title="src"]');
    if (srcRow === null) throw new Error("the src row did not render");
    await act(async () => {
      srcRow.click();
    });
    const indexRow = container.querySelector<HTMLButtonElement>(
      '.workspace-tree-file[title="src/index.ts"]',
    );
    if (indexRow === null) throw new Error("the index.ts row did not render");
    await act(async () => {
      indexRow.click();
    });
    expect(container.querySelector(".workspace-diff-card")).not.toBeNull();

    await openMenu("src");
    await menuItem("Delete");
    expect(dialogBody()).toContain("src");
    expect(dialogBody()).toContain("folder");
    await answerConfirm();

    expect(vi.mocked(workspaceFileDelete).mock.calls).toEqual([[WORKSPACE, "src"]]);
    expect(container.querySelector(".workspace-diff-card")).toBeNull();
  });

  // The confirmation's words name the entry and say what is being answered
  // for — a folder deletion takes everything inside it, and the text the
  // user confirms must say so, not just "this entry".
  it("names the entry in the confirmation, differently for a file and a folder", async () => {
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");
    const fileMessage = dialogBody();
    expect(fileMessage).toContain("README.md");
    expect(fileMessage).toContain("file");
    await answerCancel();

    await openMenu("src");
    await menuItem("Delete");
    const folderMessage = dialogBody();
    expect(folderMessage).toContain("src");
    expect(folderMessage).toContain("folder");
    expect(folderMessage).not.toBe(fileMessage);
    await answerCancel();
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
  });

  // Two levels deep the body still names the full workspace-relative
  // path, not just the basename: a message built from the entry name
  // would fail the containment below.
  it("names the full nested path in the delete question", async () => {
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) => {
      if (path === "") return Promise.resolve(listing(rootEntries));
      if (path === "src")
        return Promise.resolve(
          listing([entry("src/index.ts", "file", 6), entry("src/deep", "dir")]),
        );
      if (path === "src/deep")
        return Promise.resolve(listing([entry("src/deep/file.ts", "file", 4)]));
      if (path === "lib") return Promise.resolve(listing([entry("lib/index.ts", "file", 6)]));
      return Promise.resolve(listing([]));
    });
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    await act(async () => {
      container.querySelector<HTMLButtonElement>('.workspace-tree-dir[title="src"]')!.click();
    });
    await act(async () => {
      container.querySelector<HTMLButtonElement>('.workspace-tree-dir[title="src/deep"]')!.click();
    });

    await openMenu("src/deep/file.ts");
    await menuItem("Delete");
    expect(dialogBody()).toContain("src/deep/file.ts");
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    await answerCancel();
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  it("shows a delete refusal under the toolbar and refreshes the tree anyway", async () => {
    const refusal = "the workspace's own folder cannot be renamed, duplicated or deleted";
    vi.mocked(workspaceFileDelete).mockResolvedValue({ newPath: null, error: refusal });
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const reads = vi.mocked(workspaceFilesList).mock.calls.length;

    await openMenu("README.md");
    await menuItem("Delete");
    await answerConfirm();

    expect(alertText()).toBe(refusal);
    expect(vi.mocked(workspaceFilesList).mock.calls.length).toBe(reads + 1);
  });

  it("shows a transport failure after a confirmed delete the same way", async () => {
    vi.mocked(workspaceFileDelete).mockRejectedValue(new Error("the daemon did not answer"));
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");
    await answerConfirm();

    expect(alertText()).toBe("the daemon did not answer");
    // The parent was re-read despite the dead transport: the act may have
    // landed, and this tree has no poll to discover that later.
    const calls = vi.mocked(workspaceFilesList).mock.calls;
    expect(calls.at(-1)).toEqual([WORKSPACE, ""]);
  });
});
