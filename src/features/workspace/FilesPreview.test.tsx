// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceDirectory, WorkspaceFileContent, WorkspaceFileEntry } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFilesList: vi.fn(),
  workspaceFileRead: vi.fn(),
  reasonFromCause: vi.fn((cause: unknown) =>
    cause instanceof Error && cause.message ? cause.message : "the app did not answer",
  ),
}));

import { workspaceFileRead, workspaceFilesList } from "../../lib/tauri";
import { FilesSurface } from "./FilesSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-preview-subject";
/** A fixed stamp: the header must show THIS file's date, not "some date". */
const STAMP = 1_758_000_000_000;

function entry(path: string, kind: WorkspaceFileEntry["kind"], size: number | null = null) {
  const segments = path.split("/");
  return { path, name: segments[segments.length - 1], kind, size };
}

function listing(entries: WorkspaceFileEntry[]): WorkspaceDirectory {
  return { path: "", entries, capped: false, skipped: 0, error: null };
}

function content(overrides: Partial<WorkspaceFileContent> = {}): WorkspaceFileContent {
  return {
    status: "ok",
    kind: "text",
    content: "hello\n",
    size: 6,
    modifiedAt: STAMP,
    error: null,
    ...overrides,
  };
}

/** A promise the test settles itself, so a pending state is asserted, not raced. */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
} {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe("FilesPreview", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceFilesList).mockResolvedValue(listing([]));
    vi.mocked(workspaceFileRead).mockResolvedValue(content());
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

  /** Render a tree holding `files`, ready to be clicked. */
  async function renderFiles(...files: string[]) {
    vi.mocked(workspaceFilesList).mockResolvedValue(
      listing(files.map((path) => entry(path, "file", 6))),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);
  }

  function fileRow(path: string): HTMLButtonElement {
    const match = Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-tree-file"),
    ).find((button) => button.title === path);
    if (match === undefined) throw new Error(`file row did not render: ${path}`);
    return match;
  }

  function card(): HTMLElement {
    const match = container.querySelector<HTMLElement>(".workspace-diff-card");
    if (match === null) throw new Error("preview card did not render");
    return match;
  }

  function meta(): string {
    const spans = card().querySelectorAll(".workspace-diff-header span");
    return spans[spans.length - 1]?.textContent ?? "";
  }

  // The schedule itself — one read per click, never a read on open — plus
  // the two states between click and answer: the loading screen, then the
  // text screen with the stat's own numbers beside it. Kills the mutation
  // that reads a file without a click (a call before any row is pressed).
  it("reads a clicked file once and shows its text with size and mtime", async () => {
    const pending = deferred<WorkspaceFileContent>();
    vi.mocked(workspaceFileRead).mockReturnValue(pending.promise);
    await renderFiles("README.md");
    expect(vi.mocked(workspaceFileRead)).not.toHaveBeenCalled();

    await act(async () => {
      fileRow("README.md").click();
    });
    expect(vi.mocked(workspaceFileRead).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    expect(card().querySelector('[role="status"]')?.textContent).toBe("Loading file…");

    await act(async () => {
      pending.resolve(content());
    });
    expect(card().querySelector("pre")?.textContent).toBe("hello\n");
    expect(meta()).toContain("6 B");
    expect(meta()).toContain(new Date(STAMP).toLocaleString());

    // The same path is left untouched by a second click: re-reading is
    // Refresh's job, not a second click's.
    await act(async () => {
      fileRow("README.md").click();
    });
    expect(vi.mocked(workspaceFileRead).mock.calls).toHaveLength(1);
  });

  // The Refresh button reads the preview again — the guarantee
  // `FilesSurface.refreshAll` makes (tree **and** preview) pinned here: one
  // click on the row, one on Refresh, two reads of the same file. Kills the
  // refresh that re-reads the tree but leaves a stale preview below it.
  it("re-reads the selected file when Refresh is clicked", async () => {
    await renderFiles("README.md");
    await act(async () => {
      fileRow("README.md").click();
    });
    expect(vi.mocked(workspaceFileRead).mock.calls).toHaveLength(1);

    const refresh = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Refresh",
    );
    if (refresh === undefined) throw new Error("refresh button did not render");
    await act(async () => {
      refresh.click();
    });

    expect(vi.mocked(workspaceFileRead).mock.calls).toHaveLength(2);
    expect(vi.mocked(workspaceFileRead).mock.calls[1]).toEqual([WORKSPACE, "README.md"]);
  });

  // A folder toggles; it never asks for content — the two commands stay
  // separate even though both live under this one surface.
  it("never asks for file content when a folder row is clicked", async () => {
    // The root holds `src`, and `src` itself is empty: a mock that answers
    // every path with the same listing would make the folder its own child
    // and the tree would recurse forever — the reply must be per path.
    vi.mocked(workspaceFilesList).mockImplementation((_workspaceId, path) =>
      Promise.resolve(path === "" ? listing([entry("src", "dir")]) : listing([])),
    );
    await render(<FilesSurface workspaceId={WORKSPACE} />);

    await act(async () => {
      container.querySelector<HTMLButtonElement>(".workspace-tree-dir")?.click();
    });

    expect(vi.mocked(workspaceFileRead)).not.toHaveBeenCalled();
  });

  // A refusal is shown as the alert it is, with nothing of the file beside
  // it — the panel may then claim nothing about the bytes behind it.
  it("shows the refusal sentence as its own alert with no content", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({
        status: "refused",
        kind: null,
        content: null,
        size: null,
        modifiedAt: null,
        error: "the requested path is outside the workspace folder",
      }),
    );
    await renderFiles("escape.txt");

    await act(async () => {
      fileRow("escape.txt").click();
    });

    const alert = card().querySelector('[role="alert"]');
    expect(alert?.textContent).toBe("the requested path is outside the workspace folder");
    expect(card().querySelector("pre")).toBeNull();
    expect(meta()).toBe("refused");
  });

  // The cap's sentence carries the measure (DECISIONS-write §8: over the
  // frame's 128 KiB the panel says "too large" with the number, never a
  // slice of the file).
  it("shows too large with the measure and no content", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({
        status: "too_large",
        kind: null,
        content: null,
        size: 131073,
        error:
          "the file is larger than the 131072-byte content cap; its content is not handed back",
      }),
    );
    await renderFiles("huge.txt");

    await act(async () => {
      fileRow("huge.txt").click();
    });

    const alert = card().querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("131072-byte content cap");
    expect(card().querySelector("pre")).toBeNull();
    expect(meta()).toContain("128.0 KB");
    expect(meta()).toContain("too large");
  });

  // Binary is an answer, not a failure: its own screen, no alert, and the
  // bytes never reach the DOM in any form.
  it("shows a binary file without content and without an error", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ status: "binary", kind: "binary", content: null, size: 5 }),
    );
    await renderFiles("blob.dat");

    await act(async () => {
      fileRow("blob.dat").click();
    });

    expect(card().textContent).toContain("binary; there is no content");
    expect(card().querySelector("pre")).toBeNull();
    expect(card().querySelector("img")).toBeNull();
    expect(card().querySelector('[role="alert"]')).toBeNull();
    expect(meta()).toContain("5 B");
    expect(meta()).toContain("binary");
  });

  it("renders an image as an image under its own subtype", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ kind: "image", content: "aGVsbG8=", size: 5 }),
    );
    await renderFiles("logo.png");

    await act(async () => {
      fileRow("logo.png").click();
    });

    const image = card().querySelector("img");
    expect(image?.getAttribute("src")).toBe("data:image/png;base64,aGVsbG8=");
    expect(card().querySelector("pre")).toBeNull();
  });

  // The newest click wins: a late reply of a previously clicked file must
  // not land under the file now selected — the generation guard of
  // `useWorkspaceFilePreview`, the rule every reader of this house keeps.
  it("drops a late reply of a previously clicked file", async () => {
    const late = deferred<WorkspaceFileContent>();
    vi.mocked(workspaceFileRead).mockImplementation((_workspaceId, path) =>
      path === "a.txt" ? late.promise : Promise.resolve(content({ content: "b-content\n" })),
    );
    await renderFiles("a.txt", "b.txt");

    await act(async () => {
      fileRow("a.txt").click();
    });
    await act(async () => {
      fileRow("b.txt").click();
    });
    await act(async () => {
      late.resolve(content({ content: "stale-a\n" }));
    });

    expect(card().querySelector("pre")?.textContent).toBe("b-content\n");
    expect(card().textContent).not.toContain("stale-a");
  });

  // A failure that never became a reply (daemon down, command refused)
  // shows the sentence the bridge produced — the same screen as a wire
  // refusal, because to the panel both are "this read did not answer".
  it("shows the transport failure as its own alert", async () => {
    vi.mocked(workspaceFileRead).mockRejectedValue(new Error("the app did not answer"));
    await renderFiles("README.md");

    await act(async () => {
      fileRow("README.md").click();
    });

    expect(card().querySelector('[role="alert"]')?.textContent).toBe("the app did not answer");
    expect(card().querySelector("pre")).toBeNull();
  });
});
