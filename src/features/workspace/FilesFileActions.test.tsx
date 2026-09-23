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
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
}));

// Neither act asks for confirmation — they lose no data, so the confirmation
// belongs to delete (its own slice). The mock answers `false` on purpose:
// were a confirm() gate ever added to this road, the rename below would stop
// before its command and this file's assertions would die with it.
vi.mock("@tauri-apps/plugin-dialog", () => ({
  confirm: vi.fn(async () => false),
}));

import { confirm } from "@tauri-apps/plugin-dialog";
import {
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
    });
    vi.mocked(workspaceFileRename).mockResolvedValue({ newPath: "README.md", error: null });
    vi.mocked(workspaceFileDuplicate).mockResolvedValue({ newPath: "README copy.md", error: null });
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
});
