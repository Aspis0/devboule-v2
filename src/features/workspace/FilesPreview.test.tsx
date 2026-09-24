// @vitest-environment happy-dom

import { act } from "react";
import type { ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkspaceDirectory,
  WorkspaceFileContent,
  WorkspaceFileEntry,
  WorkspaceFileStaged,
} from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFilesList: vi.fn(),
  workspaceFileRead: vi.fn(),
  workspaceFilePreviewStage: vi.fn(),
  workspaceFilePreviewUnstage: vi.fn(),
}));

import {
  workspaceFilePreviewStage,
  workspaceFilePreviewUnstage,
  workspaceFileRead,
  workspaceFilesList,
} from "../../lib/tauri";
import { FilesSurface } from "./FilesSurface";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-preview-subject";
/** A fixed stamp: the header must show THIS file's date, not "some date". */
const STAMP = 1_758_000_000_000;
/** The asset URL of a staged copy, Windows spelling included: what
 * `convertFileSrc` hands back and what the card must draw — never a data
 * URL, never the daemon's raw path. */
const ASSET_URL = "http://asset.localhost/C%3A%5CUsers%5Cu%5Cpreviews%5Cab12.png";

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
    fromLine: 1,
    lines: 1,
    hasMore: false,
    truncated: false,
    note: null,
    ...overrides,
  };
}

/** A successful stage, the way the wrapper hands it back: URL, the kind
 * the caller asked to draw, and the source file's own stat. */
function staged(
  overrides: Partial<Extract<WorkspaceFileStaged, { status: "ok" }>> = {},
): WorkspaceFileStaged {
  return { status: "ok", url: ASSET_URL, kind: "image", size: 8, modifiedAt: STAMP, ...overrides };
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
    vi.mocked(workspaceFilePreviewStage).mockResolvedValue(staged());
    vi.mocked(workspaceFilePreviewUnstage).mockResolvedValue(undefined);
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
        fromLine: null,
        lines: null,
        hasMore: null,
        truncated: null,
        note: null,
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
        fromLine: null,
        lines: null,
        hasMore: null,
        truncated: null,
        note: null,
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
      content({
        status: "binary",
        kind: "binary",
        content: null,
        size: 5,
        fromLine: null,
        lines: null,
        hasMore: null,
        truncated: null,
        note: null,
      }),
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

  it("stages an image and draws it from the asset URL, never base64", async () => {
    vi.mocked(workspaceFilePreviewStage).mockResolvedValue(staged({ kind: "image", size: 8 }));
    await renderFiles("logo.png");

    await act(async () => {
      fileRow("logo.png").click();
    });

    // The image road is the stage, not the read: the frame's 128 KiB cap
    // is no longer anywhere near this pixel path, so the old base64
    // transport must not be asked at all.
    expect(vi.mocked(workspaceFilePreviewStage).mock.calls).toEqual([
      [WORKSPACE, "logo.png", "image"],
    ]);
    expect(vi.mocked(workspaceFileRead)).not.toHaveBeenCalled();

    const image = card().querySelector("img");
    expect(image?.getAttribute("src")).toBe(ASSET_URL);
    expect(image?.getAttribute("src") ?? "").not.toContain("data:");
    expect(card().querySelector("pre")).toBeNull();
    expect(meta()).toContain("8 B");
    expect(meta()).toContain(new Date(STAMP).toLocaleString());
  });

  // Video and PDF ride the same stage — the two extra elements the card
  // gained, each pinned by its own tag and the same URL.
  it("stages a video and draws it as a video element", async () => {
    vi.mocked(workspaceFilePreviewStage).mockResolvedValue(staged({ kind: "video", size: 4096 }));
    await renderFiles("clip.mp4");

    await act(async () => {
      fileRow("clip.mp4").click();
    });

    expect(vi.mocked(workspaceFilePreviewStage).mock.calls).toEqual([
      [WORKSPACE, "clip.mp4", "video"],
    ]);
    expect(vi.mocked(workspaceFileRead)).not.toHaveBeenCalled();
    const video = card().querySelector("video");
    expect(video?.getAttribute("src")).toBe(ASSET_URL);
    expect(card().querySelector("img")).toBeNull();
  });

  it("stages a PDF and draws it as an embed", async () => {
    vi.mocked(workspaceFilePreviewStage).mockResolvedValue(staged({ kind: "pdf" }));
    await renderFiles("manual.pdf");

    await act(async () => {
      fileRow("manual.pdf").click();
    });

    expect(vi.mocked(workspaceFilePreviewStage).mock.calls).toEqual([
      [WORKSPACE, "manual.pdf", "pdf"],
    ]);
    const embed = card().querySelector("embed");
    expect(embed?.getAttribute("src")).toBe(ASSET_URL);
    expect(embed?.getAttribute("type")).toBe("application/pdf");
  });

  // The revoke, on a selection that is not a staged file: the copy dies
  // before the new selection's own request goes out — the call order below
  // is the ordering guarantee `useWorkspaceFilePreview` builds from its
  // revoke chain.
  it("revokes the staged copy when the selection leaves it, before the next read", async () => {
    await renderFiles("logo.png", "README.md");
    await act(async () => {
      fileRow("logo.png").click();
    });
    expect(vi.mocked(workspaceFilePreviewStage)).toHaveBeenCalledTimes(1);

    await act(async () => {
      fileRow("README.md").click();
    });

    expect(vi.mocked(workspaceFilePreviewUnstage)).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceFileRead).mock.calls).toEqual([[WORKSPACE, "README.md"]]);
    expect(
      vi.mocked(workspaceFilePreviewUnstage).mock.invocationCallOrder[0],
      "the unstage must be sent before the read of the new selection",
    ).toBeLessThan(vi.mocked(workspaceFileRead).mock.invocationCallOrder[0]);
  });

  // …and on close: this panel unmounting is the panel being closed or
  // switched away, and a copy that outlives it would stay readable through
  // the asset scope until the next stage.
  it("revokes the staged copy when the panel unmounts", async () => {
    await renderFiles("logo.png");
    await act(async () => {
      fileRow("logo.png").click();
    });
    expect(vi.mocked(workspaceFilePreviewStage)).toHaveBeenCalledTimes(1);
    vi.mocked(workspaceFilePreviewUnstage).mockClear();

    await act(async () => {
      root.unmount();
    });

    expect(vi.mocked(workspaceFilePreviewUnstage)).toHaveBeenCalledTimes(1);
    // Leave a live root behind for the shared afterEach: this one is spent.
    root = createRoot(document.createElement("div"));
  });

  // A stage the daemon refused: the wire's own sentence, shown as the
  // alert it is — no element, no stat, and the card claims nothing.
  it("shows the stage's refusal sentence with no media element", async () => {
    vi.mocked(workspaceFilePreviewStage).mockResolvedValue({
      status: "refused",
      error: "the requested path is outside the workspace folder",
    });
    await renderFiles("shot.png");

    await act(async () => {
      fileRow("shot.png").click();
    });

    const alert = card().querySelector('[role="alert"]');
    expect(alert?.textContent).toBe("the requested path is outside the workspace folder");
    expect(card().querySelector("img")).toBeNull();
    expect(card().querySelector("pre")).toBeNull();
    expect(meta()).toBe("refused");
    expect(vi.mocked(workspaceFileRead)).not.toHaveBeenCalled();
  });

  // The other half of the extension mirror: `svg` is text in both lists,
  // so it goes down the read road like every other text file — a mirror
  // that drifted would stage what the daemon refuses (a sentence, not a
  // preview) or read what should have been drawn.
  it("reads an svg as text instead of staging it", async () => {
    await renderFiles("icon.svg");

    await act(async () => {
      fileRow("icon.svg").click();
    });

    expect(vi.mocked(workspaceFilePreviewStage)).not.toHaveBeenCalled();
    expect(vi.mocked(workspaceFileRead).mock.calls).toEqual([[WORKSPACE, "icon.svg"]]);
    expect(card().querySelector("pre")?.textContent).toBe("hello\n");
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

  // The windowed read, end to end at the panel: the first window shows its
  // text and its range, «Read more» asks for the next one — same file,
  // fromLine + lines — and the reply's own numbers move the range, never a
  // count of the file. Kills the mutation that sends the wrong offset (the
  // second call's args) or drops the append (the text after the click).
  it("appends the next window on Read more and shows the lines on screen", async () => {
    vi.mocked(workspaceFileRead)
      .mockResolvedValueOnce(
        content({ content: "one\ntwo\n", size: 307200, lines: 2, hasMore: true }),
      )
      .mockResolvedValueOnce(
        content({ content: "three\n", size: 307200, fromLine: 3, lines: 1, hasMore: false }),
      );
    await renderFiles("big.log");

    await act(async () => {
      fileRow("big.log").click();
    });

    expect(card().querySelector("pre")?.textContent).toBe("one\ntwo\n");
    const footer = () => card().querySelector(".workspace-file-preview-window");
    expect(footer()?.textContent).toContain("lines 1–2");
    const readMoreButton = (): HTMLButtonElement | undefined =>
      Array.from(card().querySelectorAll("button")).find(
        (button) => button.textContent === "Read more",
      );
    const more = readMoreButton();
    if (more === undefined) throw new Error("the read-more control did not render");

    await act(async () => {
      more.click();
    });

    expect(vi.mocked(workspaceFileRead).mock.calls[1]).toEqual([WORKSPACE, "big.log", 3, 5000]);
    expect(card().querySelector("pre")?.textContent).toBe("one\ntwo\nthree\n");
    expect(footer()?.textContent).toContain("lines 1–3");
    expect(readMoreButton(), "the last window offers no next one").toBeUndefined();
  });

  // The wire's own pair for a line bigger than one window: `truncated`,
  // `has_more: false` and the `note` sentence — its words are shown as
  // they arrive, and no «Read more» appears for a continuation the wire
  // says cannot resume byte-exactly. (The review called the old mock an
  // impossible state; under the line-boundary rule this is the real one.)
  it("shows the wire's sentence when a line exceeds one window", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({
        content: "y".repeat(64),
        lines: 1,
        truncated: true,
        hasMore: false,
        note: "the line exceeds one window; what follows it in the file cannot be read this way",
      }),
    );
    await renderFiles("one.log");

    await act(async () => {
      fileRow("one.log").click();
    });

    expect(card().querySelector(".workspace-file-preview-window")?.textContent).toContain(
      "the line exceeds one window",
    );
    expect(
      Array.from(card().querySelectorAll("button")).some(
        (button) => button.textContent === "Read more",
      ),
    ).toBe(false);
  });

  // A window that lands after the selection moved goes nowhere: the path
  // and generation guard of `readMore`, the gap the review named — only
  // the INITIAL read's late reply had a test until now.
  it("drops a window that arrives after another file was selected", async () => {
    const pending = deferred<WorkspaceFileContent>();
    vi.mocked(workspaceFileRead).mockImplementation((_workspaceId, path, fromLine) => {
      if (path === "big.log" && fromLine !== undefined) return pending.promise;
      if (path === "big.log") {
        return Promise.resolve(
          content({ content: "one\n", size: 307200, lines: 1, hasMore: true }),
        );
      }
      return Promise.resolve(content({ content: "b-content\n" }));
    });
    await renderFiles("big.log", "b.txt");

    await act(async () => {
      fileRow("big.log").click();
    });
    const more = Array.from(card().querySelectorAll("button")).find(
      (button) => button.textContent === "Read more",
    );
    if (more === undefined) throw new Error("the read-more control did not render");
    await act(async () => {
      more.click();
    }); // the window of big.log is still in flight

    await act(async () => {
      fileRow("b.txt").click();
    });
    expect(card().querySelector("pre")?.textContent).toBe("b-content\n");

    await act(async () => {
      pending.resolve(content({ content: "two\n", fromLine: 2, lines: 1, hasMore: false }));
    });

    expect(card().querySelector("pre")?.textContent).toBe("b-content\n");
    expect(card().textContent).not.toContain("one\ntwo");
  });
});
