// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceFileContent } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFileRead: vi.fn(),
  workspaceFilePreviewStage: vi.fn(),
  workspaceFilePreviewUnstage: vi.fn(),
  editorTargetsList: vi.fn(async () => []),
}));

import { workspaceFileRead } from "../../lib/tauri";
import { WorkspaceFileTab } from "./WorkspaceFileTab";
import { resetFileTabModeForTests } from "./fileTabMode";

import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-file-tab-subject";

function content(overrides: Partial<WorkspaceFileContent> = {}): WorkspaceFileContent {
  return {
    status: "ok",
    kind: "text",
    content: "hello\n",
    size: 6,
    modifiedAt: 1_758_000_000_000,
    error: null,
    fromLine: 1,
    lines: 1,
    hasMore: false,
    truncated: false,
    note: null,
    ...overrides,
  };
}

/** A promise the test settles itself, so a pending state is asserted, not raced. */
function deferred<T>(): {
  promise: Promise<T>;
  resolve: (value: T) => void;
} {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe("WorkspaceFileTab body", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  beforeEach(() => {
    resetFileTabModeForTests();
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceFileRead).mockResolvedValue(content());
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    container.remove();
    vi.clearAllMocks();
  });

  async function renderTab(path: string, refreshNonce = 0, key = "a") {
    const mount = (
      <WorkspaceFileTab
        key={key}
        workspaceKey={keyFor(WORKSPACE)}
        path={path}
        refreshNonce={refreshNonce}
        cache={new Map()}
      />
    );
    if (root === undefined) root = createRoot(container);
    await act(async () => {
      root!.render(mount);
    });
  }

  function sourceLines(): string {
    return Array.from(container.querySelectorAll<HTMLElement>(".workspace-file-tab-source-line"))
      .map((element) => element.textContent ?? "")
      .join("\n");
  }

  function gutterNumbers(): string[] {
    return Array.from(
      container.querySelectorAll<HTMLElement>(".workspace-file-tab-source-number"),
    ).map((element) => element.textContent ?? "");
  }

  function readMoreButton(): HTMLButtonElement | null {
    return (
      Array.from(container.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.textContent === "Read more",
      ) ?? null
    );
  }

  // ── Safety ──────────────────────────────────────────────────────────

  it("renders a Markdown image as alt text and fetches nothing", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ content: "see ![map](docs/img.png) here\n" }),
    );
    await renderTab("docs/SETUP.md");

    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".plan-markdown-image")?.textContent).toBe("map");
  });

  it("renders raw HTML in the Markdown as text", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ content: "<script>alert(1)</script>\n" }),
    );
    await renderTab("docs/SETUP.md");

    expect(container.querySelector("script")).toBeNull();
    expect(container.textContent).toContain("<script>alert(1)</script>");
  });

  it("keeps a javascript: link literal", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ content: "[open](javascript:alert(1))\n" }),
    );
    await renderTab("docs/SETUP.md");

    expect(container.querySelectorAll("a")).toHaveLength(0);
  });

  // ── Source ──────────────────────────────────────────────────────────

  it("numbers the gutter from the window's own first line", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ content: "a\nb\nc\n", fromLine: 41, lines: 3, hasMore: true }),
    );
    await renderTab("src/main.rs");

    expect(gutterNumbers()).toEqual(["41", "42", "43"]);
  });

  it("appends the next window on Read more and keeps the numbering going", async () => {
    vi.mocked(workspaceFileRead)
      .mockResolvedValueOnce(
        content({ content: "a\nb\nc\n", fromLine: 41, lines: 3, hasMore: true }),
      )
      .mockResolvedValueOnce(
        content({ content: "d\ne\n", fromLine: 44, lines: 2, hasMore: false }),
      );
    await renderTab("src/main.rs");
    const more = readMoreButton();
    if (more === null) throw new Error("Read more did not render");

    await act(async () => {
      more.click();
    });

    expect(vi.mocked(workspaceFileRead).mock.calls[1]).toEqual([
      WORKSPACE,
      "src/main.rs",
      44,
      5000,
    ]);
    expect(gutterNumbers()).toEqual(["41", "42", "43", "44", "45"]);
    expect(readMoreButton()).toBeNull();
  });

  it("shows the wire's sentence when a line was cut", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({
        content: "y".repeat(64),
        lines: 1,
        truncated: true,
        hasMore: false,
        note: "the line exceeds one window",
      }),
    );
    await renderTab("src/main.rs");

    expect(container.querySelector(".workspace-file-tab-window")?.textContent).toContain(
      "the line exceeds one window",
    );
    expect(readMoreButton()).toBeNull();
  });

  it("splits a CRLF window into clean lines and a true gutter", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(content({ content: "a\r\nb\r\n", lines: 2 }));
    await renderTab("src/main.rs");

    expect(sourceLines()).toBe("a\nb");
    expect(sourceLines()).not.toContain("\r");
    expect(gutterNumbers()).toEqual(["1", "2"]);
  });

  it("splits mixed line endings one line each", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(content({ content: "a\r\nb\nc\r", lines: 3 }));
    await renderTab("src/main.rs");

    expect(sourceLines()).toBe("a\nb\nc");
    expect(gutterNumbers()).toEqual(["1", "2", "3"]);
  });

  // ── Refresh ─────────────────────────────────────────────────────────

  it("re-reads on a re-click and keeps the old text until the new reply lands", async () => {
    const pending = deferred<WorkspaceFileContent>();
    vi.mocked(workspaceFileRead)
      .mockResolvedValueOnce(content({ content: "before\n", lines: 1 }))
      .mockReturnValueOnce(pending.promise);
    await renderTab("src/main.rs");
    expect(sourceLines()).toBe("before");

    // The already-active tab clicked again: the nonce bumps, the read goes
    // out, and the pane holds the old cell — no loading flash between.
    await renderTab("src/main.rs", 1, "a");
    expect(vi.mocked(workspaceFileRead)).toHaveBeenCalledTimes(2);
    expect(sourceLines()).toBe("before");

    await act(async () => {
      pending.resolve(content({ content: "after\n", lines: 1 }));
    });
    expect(sourceLines()).toBe("after");
  });

  // ── States ──────────────────────────────────────────────────────────

  it("shows the loading screen while the read is in flight", async () => {
    const pending = deferred<WorkspaceFileContent>();
    vi.mocked(workspaceFileRead).mockReturnValue(pending.promise);
    await renderTab("src/main.rs");

    expect(container.querySelector('[role="status"]')?.textContent).toBe("Loading file…");
    expect(container.querySelector(".workspace-file-tab-source")).toBeNull();
  });

  it("shows the wire's refusal as the alert it is", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ status: "too_large", kind: null, content: null, error: "too large for one read" }),
    );
    await renderTab("src/main.rs");

    expect(container.querySelector('[role="alert"]')?.textContent).toBe("too large for one read");
    expect(container.querySelector(".workspace-file-tab-source")).toBeNull();
  });

  it("tells a binary file's lack of content", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ status: "binary", kind: "binary", content: null, size: 4096 }),
    );
    await renderTab("blob.bin");

    expect(container.textContent).toContain("This file is binary");
    expect(container.querySelector(".workspace-file-tab-source")).toBeNull();
  });

  it("tells an ok reply whose kind has no renderer instead of a bare header", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(content({ kind: "image", content: null }));
    await renderTab("docs/notes.txt");

    expect(container.textContent).toContain("This file can't be shown here.");
    expect(container.querySelector(".workspace-file-tab-source")).toBeNull();
    expect(container.querySelector(".workspace-file-tab-preview")).toBeNull();
  });

  it("tells an ok reply with no kind the same way", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(content({ kind: null, content: null }));
    await renderTab("docs/notes.txt");

    expect(container.textContent).toContain("This file can't be shown here.");
    expect(container.querySelector(".workspace-file-tab-source")).toBeNull();
    expect(container.querySelector(".workspace-file-tab-preview")).toBeNull();
  });
});
