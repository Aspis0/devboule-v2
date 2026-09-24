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
}));

import {
  workspaceFileDuplicate,
  workspaceFileRename,
  workspaceFileRead,
  workspaceFilesList,
} from "../../lib/tauri";
import { FilesSurface } from "./FilesSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-files-subject";

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

/** A promise the test settles itself, so a pending state is asserted, not raced. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("FilesSurface", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceFilesList).mockResolvedValue(listing([]));
    // No test here clicks a file, but a default keeps an accidental click
    // from reaching `undefined` and passing for a rendered preview.
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
    // No test here acts on a row, but defaults keep an accidental click from
    // reaching `undefined` and passing for a silent no-op.
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

  /** The visible rows' labels, in DOM order — which is the reply's order. */
  function labels(): (string | null | undefined)[] {
    return Array.from(container.querySelectorAll(".workspace-tree-label")).map(
      (element) => element.textContent,
    );
  }

  function controls(): (string | null | undefined)[] {
    return Array.from(container.querySelectorAll("button")).map((button) => button.textContent);
  }

  function dirButton(path: string): HTMLButtonElement {
    const match = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-tree-dir"),
    ).find((button) => button.title === path);
    if (match === undefined) throw new Error(`folder row did not render: ${path}`);
    return match;
  }

  function refreshButton(): HTMLButtonElement {
    const match = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Refresh",
    );
    if (match === undefined) throw new Error("refresh button did not render");
    return match;
  }

  it("holds a loading state until the first answer arrives", async () => {
    const pending = deferred<WorkspaceDirectory>();
    vi.mocked(workspaceFilesList).mockReturnValue(pending.promise);
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Loading files…");
    expect(container.querySelector(".workspace-files-tree")).toBeNull();

    await act(async () => {
      pending.resolve(listing([entry("README.md", "file", 12)]));
    });
    expect(labels()).toEqual(["README.md"]);
  });

  // The schedule itself: one root read on open, and a folder read only when
  // its row opens — never a whole tree. Kills the mutation that reads every
  // folder up front (a second call where none is due yet) and pins the
  // loading row shown between the click and the answer.
  it("reads the root once and a folder only when its row expands", async () => {
    const pendingSub = deferred<WorkspaceDirectory>();
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      path === ""
        ? Promise.resolve(listing([entry("src", "dir"), entry("README.md", "file", 12)]))
        : pendingSub.promise,
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    expect(vi.mocked(workspaceFilesList).mock.calls).toEqual([[WORKSPACE, ""]]);
    const src = dirButton("src");
    expect(src.getAttribute("aria-expanded")).toBe("false");

    await act(async () => {
      src.click();
    });
    expect(vi.mocked(workspaceFilesList).mock.calls[1]).toEqual([WORKSPACE, "src"]);
    expect(src.getAttribute("aria-expanded")).toBe("true");
    expect(container.querySelector(".workspace-tree-row-note")?.textContent).toBe("Loading…");

    await act(async () => {
      pendingSub.resolve(listing([entry("src/index.rs", "file", 6)]));
    });
    expect(labels()).toContain("index.rs");

    await act(async () => {
      src.click();
    });
    expect(labels()).not.toContain("index.rs");
    expect(vi.mocked(workspaceFilesList).mock.calls).toHaveLength(2);
  });

  // Kills a client-side sort — `localeCompare` or any other: the daemon's
  // folders-first, byte-order sequence arrives once, and the panel renders
  // the order it is given instead of becoming a second authority for it.
  it("renders the daemon's order as it came, unsorted", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("src", "dir"), entry("Zeta.c", "file", 1), entry("alpha.txt", "file", 2)]),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    expect(labels()).toEqual(["src", "Zeta.c", "alpha.txt"]);
    // The other half of R2's rule, in the same breath: with `skipped: 0`
    // (the builder's default) the not-listed note must not be drawn.
    expect(container.textContent).not.toContain("not listed");
  });

  it("says the folder is empty as its own state, not an error", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(listing([]));
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    expect(container.textContent).toContain("This folder is empty.");
    expect(container.querySelector('[role="alert"]')).toBeNull();
    expect(container.querySelector(".workspace-files-tree")).toBeNull();
    expect(controls()).toContain("Refresh");
  });

  // Kills the mutation that renders the wire's refusal as an empty folder:
  // a refusal carries no entries, so the panel may claim nothing at all.
  it("shows the wire's refusal instead of claiming anything about the folder", async () => {
    const refusal = "the workspace folder is not a directory";
    vi.mocked(workspaceFilesList).mockRejectedValue(new Error(refusal));
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(refusal);
    expect(container.textContent).not.toContain("This folder is empty.");
    expect(container.querySelector(".workspace-files-tree")).toBeNull();
  });

  // The Changes panel's rule R1, here: a refresh that did not answer may not
  // hide the list the user was looking at. R3 of the fix round pins the
  // ORDER while here: the alert stands ABOVE the list that stays — the
  // report once claimed the opposite, and the DOM position is now proved.
  it("keeps the list on screen under the sentence of a refresh that failed", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("src", "dir"), entry("README.md", "file", 12)]),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    vi.mocked(workspaceFilesList).mockRejectedValue(new Error("the daemon refused this read"));

    await act(async () => {
      refreshButton().click();
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toBe(
      "the daemon refused this read",
    );
    expect(labels()).toEqual(["src", "README.md"]);
    const alert = container.querySelector('[role="alert"]');
    const tree = container.querySelector(".workspace-files-tree");
    if (alert === null || tree === null) throw new Error("alert and tree must both be on screen");
    expect(alert.compareDocumentPosition(tree) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });

  // A folder's own refusal lives under its row: the root answered, so the
  // panel-level error box must stay empty — one failure, one place.
  it("shows a folder's own refusal under its row, beside the rest of the tree", async () => {
    const refusal = "the folder could not be listed";
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      path === ""
        ? Promise.resolve(listing([entry("src", "dir"), entry("README.md", "file", 12)]))
        : Promise.reject(new Error(refusal)),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await act(async () => {
      dirButton("src").click();
    });

    const row = container.querySelector(".workspace-tree-row-note-error");
    expect(row?.textContent).toBe(refusal);
    expect(row?.getAttribute("role")).toBe("alert");
    expect(container.querySelector(".workspace-files-error")).toBeNull();
    expect(labels()).toEqual(["src", "README.md"]);
  });

  // Kills the mutation that drops `capped`: a partial listing says so in
  // its own row instead of passing for the whole folder.
  it("declares a capped listing in the tree", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("a.txt", "file", 1), entry("b.txt", "file", 1)], { capped: true }),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    const partial = Array.from(container.querySelectorAll('[role="status"]')).find((note) =>
      note.textContent?.includes("the list is partial"),
    );
    expect(partial).toBeDefined();
    expect(container.textContent).toContain("a.txt");
  });

  // Kills the mutation that drops the `skipped` count (R2): a folder whose
  // daemon skipped entries — links, entries that would not stat — declares
  // the number instead of looking complete; the note is drawn only when the
  // count is > 0 (its absence at 0 is pinned in the order test above).
  it("declares how many entries were not listed when the daemon skipped some", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("real.txt", "file", 4)], { skipped: 2 }),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    expect(container.textContent).toContain("real.txt");
    const note = Array.from(container.querySelectorAll('[role="status"]')).find((element) =>
      element.textContent?.includes("not listed"),
    );
    expect(note?.textContent).toContain("2 entries are not listed");
  });

  it("re-reads the root and every expanded folder on the manual refresh", async () => {
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      Promise.resolve(
        path === "" ? listing([entry("src", "dir")]) : listing([entry("src/index.rs", "file", 6)]),
      ),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);
    await act(async () => {
      dirButton("src").click();
    });
    const reads = (path: string) =>
      vi.mocked(workspaceFilesList).mock.calls.filter((call) => call[1] === path).length;
    expect(reads("")).toBe(1);
    expect(reads("src")).toBe(1);

    await act(async () => {
      refreshButton().click();
    });

    expect(reads("")).toBe(2);
    expect(reads("src")).toBe(2);
  });

  // The doctrine's shape after the delete slice: the row menu carries the
  // three acts — the two that lose no data, and the delete, which loses
  // data and is therefore the only one gated (its confirmation is asserted
  // in FilesFileActions.test.tsx, where the wire can be watched). The
  // absences that remain are create, download (the phone workstream's),
  // and the Changes panel's stage/discard, which belong to that panel.
  // Anchored on the real reply's names first, so the absences cannot pass
  // on an empty panel.
  it("offers the two acts that lose no data plus the gated delete, and none beyond", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing([entry("crates", "dir"), entry("real-file.rs", "file", 2048)]),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    expect(container.textContent).toContain("real-file.rs");
    expect(container.textContent).toContain("2.0 KB");
    expect(container.textContent).not.toContain("Mockup");
    expect(container.querySelector('[role="note"]')).toBeNull();
    expect(container.querySelectorAll(".workspace-tree-file")).toHaveLength(1);
    // With every menu closed, the buttons on screen are only the refresh
    // control, the folder toggles, the file rows and the rows' own menu
    // triggers — how the panel reads and how it reaches its three acts.
    for (const button of Array.from(container.querySelectorAll("button"))) {
      expect(
        button.classList.contains("workspace-tree-dir") ||
          button.classList.contains("workspace-tree-file") ||
          button.classList.contains("workspace-tree-menu-trigger") ||
          button.textContent === "Refresh",
      ).toBe(true);
    }
    // The acts exist: open the file row's menu and they are named there.
    const trigger = container.querySelector<HTMLButtonElement>(".workspace-tree-menu-trigger");
    if (trigger === null) throw new Error("the row's menu trigger did not render");
    await act(async () => {
      trigger.click();
    });
    const menuItems = Array.from(
      container.querySelectorAll<HTMLButtonElement>('[role="menuitem"]'),
    ).map((item) => item.textContent);
    expect(menuItems).toEqual(["Rename", "Duplicate", "Delete"]);
    // And the absences this panel keeps: the rest never lands here — and
    // the gated delete is the LAST item, after the two harmless acts.
    for (const forbidden of ["New", "Download", "Stage", "Discard"]) {
      expect(menuItems).not.toContain(forbidden);
      expect(controls()).not.toContain(forbidden);
    }
    expect(container.textContent).not.toContain("No workspace file tree is read yet");
  });

  it("asks nothing while no workspace is selected and says so", async () => {
    await render(<FilesSurface workspaceId={null} />);

    expect(vi.mocked(workspaceFilesList)).not.toHaveBeenCalled();
    expect(container.textContent).toContain("No workspace is selected.");
    expect(controls()).not.toContain("Refresh");
    expect(container.querySelector(".workspace-files-toolbar")).toBeNull();
  });

  // Same rule as the Changes panel's R2: a reply of the previous workspace
  // may not appear under the new one's name.
  it("shows nothing of the previous workspace once the id changes", async () => {
    const pendingB = deferred<WorkspaceDirectory>();
    vi.mocked(workspaceFilesList).mockImplementation((workspaceId, path) => {
      if (workspaceId === "workspace-files-a" && path === "") {
        return Promise.resolve(listing([entry("a-only.txt", "file", 1)]));
      }
      if (workspaceId === "workspace-files-b" && path === "") return pendingB.promise;
      return Promise.resolve(listing([]));
    });
    await render(<FilesSurface workspaceId="workspace-files-a" />);
    expect(labels()).toEqual(["a-only.txt"]);

    await act(async () => {
      root.render(<FilesSurface workspaceId="workspace-files-b" />);
    });
    expect(container.textContent).toContain("Loading files…");
    expect(container.textContent).not.toContain("a-only.txt");

    await act(async () => {
      pendingB.resolve(listing([]));
    });
    expect(container.textContent).toContain("This folder is empty.");
    expect(container.textContent).not.toContain("a-only.txt");
  });
});
