// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceDirectory, WorkspaceFileEntry } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFilesList: vi.fn(),
  workspaceFileRead: vi.fn(),
}));

import { workspaceFileRead, workspaceFilesList } from "../../lib/tauri";
import { FilesSurface } from "./FilesSurface";
import { assembleCssProof, removeCssProof } from "./cssProof";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

/** How many folder sorts the panel ran: the memo test's only observable.
 * Reset per test; the wrapper calls through, so the order assertions keep
 * proving the contract while this counts its cost. */
let sortCalls = 0;

vi.mock("./filesSort", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./filesSort")>();
  return {
    ...actual,
    sortFileEntries: (entries: readonly WorkspaceFileEntry[]) => {
      sortCalls += 1;
      return actual.sortFileEntries(entries);
    },
  };
});

/** How many file rows rendered a size: one call per rendered file row
 * that carries one. Reset per test; the wrapper calls through, so the
 * sizes on screen stay the stat's own numbers while this counts rebuilt
 * rows. */
let sizeCalls = 0;

vi.mock("./FilesPreview", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./FilesPreview")>();
  return {
    ...actual,
    formatSize: (bytes: number) => {
      sizeCalls += 1;
      return actual.formatSize(bytes);
    },
  };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const WORKSPACE = "workspace-files-sort-subject";

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

describe("FilesSurface R7c sort and toolbar", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = undefined;
    vi.mocked(workspaceFilesList).mockResolvedValue(listing([]));
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
  });

  beforeEach(() => {
    sortCalls = 0;
    sizeCalls = 0;
  });

  afterEach(async () => {
    const current = root;
    if (current !== undefined) {
      await act(async () => {
        current.unmount();
      });
    }
    container.remove();
    removeCssProof();
    vi.clearAllMocks();
  });

  async function render(ui: ReactNode) {
    const previous = root;
    if (previous !== undefined) {
      await act(async () => {
        previous.unmount();
      });
    }
    root = createRoot(container);
    const current = root;
    await act(async () => {
      current.render(ui);
    });
  }

  /** The visible rows' labels, in DOM order — which must be the panel's
   * sorted order, not the reply's. */
  function labels(): (string | null | undefined)[] {
    return Array.from(container.querySelectorAll(".workspace-tree-label")).map(
      (element) => element.textContent,
    );
  }

  function dirButton(path: string): HTMLButtonElement {
    const match = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-tree-dir"),
    ).find((button) => button.title === path);
    if (match === undefined) throw new Error(`folder row did not render: ${path}`);
    return match;
  }

  // The revoked rule's replacement (owner, 2026-09-26 night): the panel
  // sorts client-side, folders first always. The daemon's folders-first
  // byte order arrives once; the panel is the second authority by decision.
  // This reply is scrambled on purpose — a file first, the folder last —
  // so the daemon's order can never pass for the panel's.
  it("sorts folders first, then files by name", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("Zeta.c", "file", 1), entry("alpha.txt", "file", 2), entry("src", "dir")]),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);

    expect(labels()).toEqual(["src", "alpha.txt", "Zeta.c"]);
  });

  // The panel collation, pinned: `en`, base sensitivity, numeric. Case
  // folds (Zeta sorts after alpha), numbers run naturally (a2 before
  // a10), accents fold to their base (éclair with e). Kills the byte-order
  // sort the daemon itself uses, which would put every uppercase first.
  it("orders names with the panel collation, not byte order", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([
        entry("Zeta.c", "file", 1),
        entry("README.md", "file", 2),
        entry("éclair.md", "file", 3),
        entry("a10.txt", "file", 4),
        entry("a2.txt", "file", 5),
      ]),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);

    expect(labels()).toEqual(["a2.txt", "a10.txt", "éclair.md", "README.md", "Zeta.c"]);
  });

  // Equal under the collation (`A.txt` vs `a.txt` at base sensitivity) is
  // the daemon's order kept: the sort is stable, never a coin toss.
  // A guard more than a red test — the old code rendered the reply's order
  // too, so this passes before and after; it kills the unstable-sort
  // mutation (an index-keyed shuffle) instead.
  it("keeps the daemon's order for names the collation calls equal", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("a.txt", "file", 1), entry("A.txt", "file", 2)]),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);

    expect(labels()).toEqual(["a.txt", "A.txt"]);
  });

  // The sort is per folder, not root-only: an expanded folder's own reply
  // is ordered the same way before it renders.
  it("sorts an expanded folder's own entries the same way", async () => {
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      path === ""
        ? Promise.resolve(listing([entry("src", "dir")]))
        : Promise.resolve(listing([entry("src/z.txt", "file", 1), entry("src/a.txt", "file", 2)])),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);
    await act(async () => {
      dirButton("src").click();
    });

    expect(labels()).toEqual(["src", "a.txt", "z.txt"]);
  });

  // The sort is memoised on the entries reference (F-05): a reply is
  // immutable once it lands, so opening a menu and typing a rename —
  // re-renders with the same references — must not re-sort. Kills the
  // plain-closure FolderGroup that re-sorts every folder per keystroke.
  it("does not re-sort folders while a menu opens and a rename is typed", async () => {
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      Promise.resolve(
        path === ""
          ? listing([entry("src", "dir"), entry("b.txt", "file", 1)])
          : listing([entry("src/z.txt", "file", 1), entry("src/a.txt", "file", 2)]),
      ),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);
    await act(async () => {
      dirButton("src").click();
    });
    const settled = sortCalls;
    expect(settled).toBeGreaterThan(0);

    const trigger = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-tree-menu-trigger"),
    ).find((button) => button.getAttribute("aria-label") === "a.txt actions");
    if (trigger === undefined) throw new Error("row menu trigger did not render");
    await act(async () => {
      trigger.click();
    });
    expect(sortCalls).toBe(settled);

    const rename = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((item) => item.textContent === "Rename");
    if (rename === undefined) throw new Error("Rename item did not render");
    await act(async () => {
      rename.click();
    });
    const input = container.querySelector<HTMLInputElement>(".workspace-tree-rename");
    if (input === null) throw new Error("rename input did not render");
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");
    await act(async () => {
      setter.call(input, "a2.txt");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(sortCalls).toBe(settled);
  });

  // A rename keystroke rebuilds one row, not the tree (N-01): the
  // renaming state reaches only the row being renamed as a value pair,
  // so every other memo'd row keeps identical props. Kills the object
  // prop that defeated the memo for 39 of 40 rows per character typed.
  it("rebuilds only the renaming row on a rename keystroke", async () => {
    const files = Array.from({ length: 40 }, (_, index) => `f${index}.txt`);
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing(files.map((path) => entry(path, "file", 6))),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);
    expect(sizeCalls).toBe(40);

    const trigger = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-tree-menu-trigger"),
    ).find((button) => button.getAttribute("aria-label") === "f0.txt actions");
    if (trigger === undefined) throw new Error("row menu trigger did not render");
    await act(async () => {
      trigger.click();
    });
    const rename = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).find((item) => item.textContent === "Rename");
    if (rename === undefined) throw new Error("Rename item did not render");
    await act(async () => {
      rename.click();
    });
    const input = container.querySelector<HTMLInputElement>(".workspace-tree-rename");
    if (input === null) throw new Error("rename input did not render");
    const settled = sizeCalls;
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    if (setter === undefined) throw new Error("no value setter");
    await act(async () => {
      setter.call(input, "f0-renamed.txt");
      input.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(input.value).toBe("f0-renamed.txt");
    expect(sizeCalls).toBe(settled);
  });

  // One criterion means no menu (F-03): the toolbar names the order with
  // a static label — the mockup's own `.ftoolbar` shape — instead of a
  // control that cannot do anything. Clicking it opens nothing.
  it("names the order with a static label and offers no menu", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("b.txt", "file", 1), entry("a.txt", "file", 2)]),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);

    expect(labels()).toEqual(["a.txt", "b.txt"]);
    const label = container.querySelector(".workspace-files-sort-label");
    if (label === null) throw new Error("sort label did not render");
    expect(label.textContent).toContain("Name");
    expect(label.tagName).not.toBe("BUTTON");
    expect(container.querySelector('[role="menu"]')).toBeNull();
    await act(async () => {
      label.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(container.querySelector('[role="menu"]')).toBeNull();
    expect(labels()).toEqual(["a.txt", "b.txt"]);
  });

  // The refresh is a quiet icon button, as R7b's: no text control anywhere
  // in the toolbar, one labelled icon that re-reads tree and preview.
  it("refreshes from a quiet icon button, never a text control", async () => {
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);

    expect(
      Array.from(container.querySelectorAll("button")).some(
        (candidate) => candidate.textContent === "Refresh",
      ),
    ).toBe(false);
    const refresh = container.querySelector<HTMLButtonElement>(".workspace-files-refresh");
    if (refresh === null) throw new Error("refresh icon button did not render");
    expect(refresh.getAttribute("aria-label")).toBe("Refresh");
    expect(refresh.textContent).not.toContain("Refresh");

    await act(async () => {
      refresh.click();
    });
    expect(vi.mocked(workspaceFilesList)).toHaveBeenCalledTimes(2);
  });

  // Disclosure-list semantics, as R7b chose: no role="tree" (rows carry a
  // menu trigger each, so the single-tab-stop pattern cannot hold), every
  // expanded folder's toggle naming the group it owns — and only while it
  // owns one. Arrow keys belong to the focused control itself.
  it("renders folders as disclosures owning their groups, with no tree role", async () => {
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      path === ""
        ? Promise.resolve(listing([entry("src", "dir")]))
        : Promise.resolve(listing([entry("src/a.txt", "file", 1)])),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);
    await act(async () => {
      dirButton("src").click();
    });

    expect(container.querySelector('[role="tree"]')).toBeNull();
    const tree = container.querySelector(".workspace-files-tree");
    if (tree === null) throw new Error("tree did not render");
    expect(tree.tagName).toBe("UL");
    const toggle = dirButton("src");
    const groupId = toggle.getAttribute("aria-controls");
    if (groupId === null) throw new Error("folder toggle names no group");
    expect(container.querySelector(`#${CSS.escape(groupId)}`)?.tagName).toBe("UL");

    await act(async () => {
      toggle.click();
    });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    expect(toggle.getAttribute("aria-controls")).toBeNull();
  });

  // Slice 8's hand-off: the selected file carries a pencil that opens it
  // as a main tab through one callback — the workspace id plus the
  // workspace-relative path, never an absolute path. No callback, no
  // pencil: a control with no destination is a lie.
  it("hands the selected file to slice 8 as id plus relative path", async () => {
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      Promise.resolve(
        path === ""
          ? listing([entry("docs", "dir")])
          : listing([entry("docs/SETUP.md", "file", 6)]),
      ),
    );
    const onOpenFile = vi.fn();
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} onOpenFile={onOpenFile} />);
    expect(container.querySelector('[aria-label="Open file in a tab"]')).toBeNull();
    await act(async () => {
      dirButton("docs").click();
    });

    const file = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-tree-file"),
    ).find((button) => button.title === "docs/SETUP.md");
    if (file === undefined) throw new Error("file row did not render");
    await act(async () => {
      file.click();
    });
    const pencil = container.querySelector<HTMLButtonElement>('[aria-label="Open file in a tab"]');
    if (pencil === null) throw new Error("pencil did not render on the selected row");
    await act(async () => {
      pencil.click();
    });

    expect(onOpenFile).toHaveBeenCalledWith(keyFor(WORKSPACE), "docs/SETUP.md");
    const [, path] = onOpenFile.mock.calls[0] as [WorkspaceKey, string];
    expect(path.startsWith("/")).toBe(false);
    expect(path).not.toContain(":");
    expect(path).not.toContain("\\");
  });

  it("shows no pencil while slice 8 has no callback", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(listing([entry("a.txt", "file", 1)]));
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} />);

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-tree-file")?.click();
    });
    expect(container.querySelector('[aria-label="Open file in a tab"]')).toBeNull();
  });

  // The mockup's Files chrome from the sheets that can name these
  // selectors, in the order the production bundle emits them (measured
  // with `vite build`: index.css — tokens, global, errorBoundaries,
  // PermissionCard, PickerChip — then artifactPreview, then Workspace.css
  // in the SidebarFooter chunk, then the Workspace chunk as changes →
  // files → QueueTrack → strip → history → sidebar → panel.css; the
  // settings/design/marketplace/polis/oracle chunks load after. Every
  // other sheet in the repo names none of these selectors, verified by
  // grep — the omissions below that line are inert, not unexamined). The toolbar (sort label
  // left, quiet refresh right), h24 sans rows with the 14px indent step,
  // the trigger fitting its row, the name beside its icon, mockup radii
  // and sizes, the selected file as a fill-tool row, and the preview
  // card's header as UI text — only the file's own bytes keep mono.
  it("paints toolbar, rows and preview header from the sheets in bundle order", async () => {
    const { inject, token } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/app/errorBoundaries.css"),
      read("src/components/PermissionCard.css"),
      read("src/components/PickerChip.css"),
      read("src/features/design/artifactPreview.css"),
      read("src/features/workspace/Workspace.css"),
      read("src/features/workspace/panel/changes.css"),
      read("src/features/workspace/panel/files.css"),
      read("src/features/workspace/QueueTrack.css"),
      read("src/features/workspace/strip/strip.css"),
      read("src/features/history/history.css"),
      read("src/features/workspace/sidebar/sidebar.css"),
      read("src/features/workspace/panel/panel.css"),
    ]);
    inject([
      ".workspace-files-toolbar",
      ".workspace-files-sort-label",
      ".workspace-files-refresh",
      ".workspace-files-tree",
      ".workspace-files-row",
      ".workspace-tree-dir",
      ".workspace-tree-file",
      ".workspace-files-file-icon",
      ".workspace-tree-chevron",
      ".workspace-tree-size",
      ".workspace-files-row .workspace-tree-size",
      ".workspace-files-row .workspace-tree-chevron",
      ".workspace-tree-menu-trigger",
      ".workspace-files .workspace-tree-menu-trigger",
      ".workspace-files-selected",
      ".workspace-files-pencil",
      ".workspace-diff-header",
      ".workspace-files .workspace-diff-header",
    ]);
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      Promise.resolve(
        path === "" ? listing([entry("src", "dir")]) : listing([entry("src/a.txt", "file", 2048)]),
      ),
    );
    await render(<FilesSurface workspaceKey={keyFor(WORKSPACE)} onOpenFile={() => undefined} />);

    const toolbar = container.querySelector<HTMLElement>(".workspace-files-toolbar");
    if (toolbar === null) throw new Error("toolbar did not render");
    expect(getComputedStyle(toolbar).display).toBe("flex");
    const refresh = container.querySelector<HTMLElement>(".workspace-files-refresh");
    if (refresh === null) throw new Error("refresh icon button did not render");
    expect(getComputedStyle(refresh).width).toBe("24px");
    expect(getComputedStyle(refresh).height).toBe("24px");

    const folder = dirButton("src");
    expect(getComputedStyle(folder).height).toBe("24px");
    expect(getComputedStyle(folder).fontSize).toBe("12px");
    expect(getComputedStyle(folder).fontFamily).not.toContain("JetBrains Mono");
    expect(getComputedStyle(folder).paddingLeft).toBe("6px");
    // The mockup's 6px radius, not the moved bone's 8px.
    expect(getComputedStyle(folder).borderRadius).toBe("6px");
    // The trigger fits its row: 24px, so the tallest child never makes
    // the row taller than the button the proof measures above.
    const trigger = container.querySelector<HTMLElement>(".workspace-tree-menu-trigger");
    if (trigger === null) throw new Error("row menu trigger did not render");
    expect(getComputedStyle(trigger).height).toBe("24px");

    await act(async () => {
      folder.click();
    });
    const file = container.querySelector<HTMLElement>('.workspace-tree-file[title="src/a.txt"]');
    if (file === null) throw new Error("nested file row did not render");
    // One 14px step below its folder's 6px pad.
    expect(getComputedStyle(file).paddingLeft).toBe("20px");
    // The name sits beside its icon: no space-between, the size pushed
    // right with a margin instead.
    expect(getComputedStyle(file).justifyContent).toBe("flex-start");
    const icon = file.querySelector<HTMLElement>(".workspace-files-file-icon");
    if (icon === null) throw new Error("file icon did not render");
    expect(getComputedStyle(icon).width).toBe("12px");
    const size = file.querySelector<HTMLElement>(".workspace-tree-size");
    if (size === null) throw new Error("file size did not render");
    expect(getComputedStyle(size).marginLeft).toBe("auto");
    expect(getComputedStyle(size).fontSize).toBe("12px");

    await act(async () => {
      file.click();
    });
    expect(getComputedStyle(file).backgroundColor).toBe(token("--fill-tool"));
    const header = container.querySelector<HTMLElement>(".workspace-diff-header");
    if (header === null) throw new Error("preview header did not render");
    expect(getComputedStyle(header).fontFamily).not.toContain("JetBrains Mono");
  });
});
