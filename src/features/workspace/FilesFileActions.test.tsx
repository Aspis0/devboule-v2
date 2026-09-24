// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceDirectory, WorkspaceFileEntry } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFilesList: vi.fn(),
  workspaceFileRead: vi.fn(),
  workspaceFileRename: vi.fn(),
  workspaceFileDuplicate: vi.fn(),
  workspaceFileDelete: vi.fn(),
}));

// The confirmation belongs to the one act that loses data: the delete's gate
// lives inside `deleteEntry`, and this mock answers `false` on purpose —
// were the gate ever dropped from that road, the No-answers-nothing case
// below would die first. The two acts that lose no data must never reach it
// at all, which their own tests assert.
vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(async () => false),
}));

import { confirm } from "@tauri-apps/plugin-dialog";
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
    vi.clearAllMocks();
  });

  async function render(ui: ReactNode) {
    root = createRoot(container);
    await act(async () => {
      root.render(ui);
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

  async function pressKey(key: string): Promise<void> {
    await act(async () => {
      renameInput().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
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
    expect(vi.mocked(confirm)).not.toHaveBeenCalled();
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
    expect(vi.mocked(confirm)).not.toHaveBeenCalled();
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
    expect(vi.mocked(confirm)).not.toHaveBeenCalled();
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
    expect(vi.mocked(confirm)).not.toHaveBeenCalled();
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
      '.workspace-tree-file[aria-pressed="true"]',
    );
    expect(selected?.getAttribute("title")).toBe("GUIDE.md");
  });

  // The slice's own shape (plan-write §2.4): a No at the confirmation stops
  // EVERYTHING — no command reaches the wire, and the tree is not even
  // re-read, because nothing happened to re-read. Kills the mutation that
  // drops the confirmation gate from `deleteEntry`.
  it("asks before deleting, and a No stops everything before the wire", async () => {
    vi.mocked(confirm).mockResolvedValue(false);
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");
    await act(async () => undefined);

    expect(vi.mocked(confirm)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceFilesList).mock.calls).toEqual([[WORKSPACE, ""]]);
    expect(container.textContent).toContain("README.md");
    expect(alertText()).toBeNull();
  });

  it("deletes once confirmed: the parent is re-read and a dead selection drops its preview", async () => {
    vi.mocked(confirm).mockResolvedValue(true);
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
    await act(async () => undefined);

    expect(vi.mocked(workspaceFileDelete).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    expect(container.textContent).not.toContain("README.md");
    // The preview died with the file: no card still showing bytes of it, no
    // row left pressed, and no re-read of the dead path ever issued.
    expect(container.querySelector(".workspace-diff-card")).toBeNull();
    expect(container.querySelector('[aria-pressed="true"]')).toBeNull();
    expect(vi.mocked(workspaceFileRead).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    expect(alertText()).toBeNull();
  });

  it("deleting a folder takes the selection under it and names the folder in the confirmation", async () => {
    vi.mocked(confirm).mockResolvedValue(true);
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
    await act(async () => undefined);

    expect(vi.mocked(workspaceFileDelete).mock.calls).toEqual([[WORKSPACE, "src"]]);
    expect(container.querySelector(".workspace-diff-card")).toBeNull();
  });

  // The confirmation's words name the entry and say what is being answered
  // for — a folder deletion takes everything inside it, and the text the
  // user confirms must say so, not just "this entry".
  it("names the entry in the confirmation, differently for a file and a folder", async () => {
    vi.mocked(confirm).mockResolvedValue(false);
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");
    const fileCalls = vi.mocked(confirm).mock.calls;
    expect(fileCalls).toHaveLength(1);
    const fileMessage = fileCalls[0][0];
    expect(fileMessage).toContain("README.md");
    expect(fileMessage).toContain("file");

    await openMenu("src");
    await menuItem("Delete");
    const folderCalls = vi.mocked(confirm).mock.calls;
    expect(folderCalls).toHaveLength(2);
    const folderMessage = folderCalls[1][0];
    expect(folderMessage).toContain("src");
    expect(folderMessage).toContain("folder");
    expect(folderMessage).not.toBe(fileMessage);
    expect(vi.mocked(workspaceFileDelete)).not.toHaveBeenCalled();
  });

  it("shows a delete refusal under the toolbar and refreshes the tree anyway", async () => {
    const refusal = "the workspace's own folder cannot be renamed, duplicated or deleted";
    vi.mocked(confirm).mockResolvedValue(true);
    vi.mocked(workspaceFileDelete).mockResolvedValue({ newPath: null, error: refusal });
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    const reads = vi.mocked(workspaceFilesList).mock.calls.length;

    await openMenu("README.md");
    await menuItem("Delete");
    await act(async () => undefined);

    expect(alertText()).toBe(refusal);
    expect(vi.mocked(workspaceFilesList).mock.calls.length).toBe(reads + 1);
  });

  it("shows a transport failure after a confirmed delete the same way", async () => {
    vi.mocked(confirm).mockResolvedValue(true);
    vi.mocked(workspaceFileDelete).mockRejectedValue(new Error("the daemon did not answer"));
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await openMenu("README.md");
    await menuItem("Delete");
    await act(async () => undefined);

    expect(alertText()).toBe("the daemon did not answer");
    // The parent was re-read despite the dead transport: the act may have
    // landed, and this tree has no poll to discover that later.
    const calls = vi.mocked(workspaceFilesList).mock.calls;
    expect(calls.at(-1)).toEqual([WORKSPACE, ""]);
  });
});
