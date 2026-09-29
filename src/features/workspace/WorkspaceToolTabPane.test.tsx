// @vitest-environment happy-dom

// Tool panes re-read beside their panel copies: error detail ids stay per
// instance, a re-clicked tab renews its content without flashing empty, and
// the Files pencil offers no tab for a staged preview.

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  chipClick,
  flush,
  plainClick,
  renderWorkspace,
  tabElement,
} from "./bulkCloseHarness";
import {
  workspaceFileRead,
  workspaceFilesList,
  workspaceGitDiff,
  workspaceGitStatus,
} from "../../lib/tauri";
import { toolTabId } from "./strip/toolTabs";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

function statusWithRow(path: string) {
  return {
    isGit: true,
    dirty: true,
    branch: "main",
    totals: { additions: 3, deletions: 1 },
    rows: [
      {
        path,
        renamedFrom: null,
        additions: 3,
        deletions: 1,
        status: "modified" as const,
        capped: false,
      },
    ],
    error: null,
  };
}

function filesWithEntry(path: string, name: string) {
  return {
    path: "",
    entries: [{ path, name, kind: "file" as const, size: 13 }],
    capped: false,
    skipped: 0,
    error: null,
  };
}

/** Every diff surface's error: its described-by target and its own detail node.
    The tab is its own surface since C10 (`.diff-tab`); the panel cards stay
    `.workspace-diff-card`. */
function diffSurfaceErrorLinks(): Array<{ describedBy: string | null; detailId: string | null }> {
  return [...document.querySelectorAll(".workspace-diff-card, .diff-tab")].map((card) => ({
    describedBy: card.querySelector("[aria-describedby]")?.getAttribute("aria-describedby") ?? null,
    detailId: card.querySelector(".error-detail-sr-only")?.getAttribute("id") ?? null,
  }));
}

function diffReply(text: string) {
  return {
    path: "src/writer.ts",
    isNew: false,
    isDeleted: false,
    additions: 1,
    deletions: 0,
    lines: [{ kind: "add" as const, text }],
    status: "ok" as const,
    error: null,
  };
}

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

/** A diff tab left active, showing the mocked content. */
async function openActiveDiffTab(): Promise<string> {
  vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow("src/writer.ts"));
  await renderWorkspace();
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
  });
  await flush();
  await act(async () => {
    document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
  });
  await flush();
  const id = toolTabId("diff", "workspace-1", "src/writer.ts");
  expect(tabElement(id).getAttribute("aria-selected")).toBe("true");
  return id;
}

function tabPanelText(): string {
  return document.querySelector("#workspace-panel-terminal")?.textContent ?? "";
}

describe("re-reading a tool tab", () => {
  it("a click on the active diff tab renews its content", async () => {
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const first = 1;"));
    const id = await openActiveDiffTab();
    expect(tabPanelText()).toContain("const first = 1;");
    const callsAfterOpen = vi.mocked(workspaceGitDiff).mock.calls.length;

    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const second = 2;"));
    await plainClick(id);
    await flush();

    expect(vi.mocked(workspaceGitDiff).mock.calls.length).toBeGreaterThan(callsAfterOpen);
    expect(tabPanelText()).toContain("const second = 2;");
  });

  it("the re-read keeps the old body until the new read lands", async () => {
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const first = 1;"));
    const id = await openActiveDiffTab();
    const gate = deferred<ReturnType<typeof diffReply>>();
    vi.mocked(workspaceGitDiff).mockReturnValue(gate.promise);

    await plainClick(id);
    await flush();

    expect(tabPanelText()).toContain("const first = 1;");
    expect(tabPanelText()).not.toContain("Loading diff");
    gate.resolve(diffReply("const second = 2;"));
    await flush();
    expect(tabPanelText()).toContain("const second = 2;");
  });

  it("a reply that lands after leaving the tab is dropped", async () => {
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const first = 1;"));
    const id = await openActiveDiffTab();
    const gate = deferred<ReturnType<typeof diffReply>>();
    vi.mocked(workspaceGitDiff).mockReturnValue(gate.promise);

    await plainClick(id);
    await flush();
    await plainClick("session-2");
    await flush();
    gate.resolve(diffReply("const stale = 9;"));
    await flush();
    await flush();

    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const third = 3;"));
    await plainClick(id);
    await flush();
    // The dropped reply never showed: back on the tab, the kept first body
    // stood until the new read landed.
    expect(tabPanelText()).toContain("const third = 3;");
    expect(tabPanelText()).not.toContain("const stale = 9;");
  });

  it("a click on the active file tab renews its preview", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry("docs/SETUP.md", "SETUP.md"));
    await renderWorkspace();
    await act(async () => {
      document.querySelector<HTMLElement>('[data-panel-tab="files"]')?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-tree-file")?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open file in a tab"]')?.click();
    });
    await flush();
    const id = toolTabId("file", "workspace-1", "docs/SETUP.md");
    expect(tabPanelText()).toContain("preview bytes");

    vi.mocked(workspaceFileRead).mockResolvedValue({
      status: "ok",
      kind: "text",
      content: "preview bytes v2",
      size: 16,
      modifiedAt: null,
      error: null,
      fromLine: 1,
      lines: 1,
      hasMore: false,
      truncated: false,
      note: null,
    });
    await plainClick(id);
    await flush();
    expect(tabPanelText()).toContain("preview bytes v2");
  });
});

describe("content cache", () => {
  it("closing a diff tab forgets what it showed", async () => {
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const first = 1;"));
    const id = await openActiveDiffTab();
    expect(tabPanelText()).toContain("const first = 1;");

    await chipClick(id);
    await flush();
    expect(document.querySelector(`#${CSS.escape(`workspace-session-tab-${id}`)}`)).toBeNull();

    const gate = deferred<ReturnType<typeof diffReply>>();
    vi.mocked(workspaceGitDiff).mockReturnValue(gate.promise);
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
    });
    await flush();
    // No seed: the evicted tab re-reads from empty, never the old body.
    expect(tabPanelText()).not.toContain("const first = 1;");
    gate.resolve(diffReply("const second = 2;"));
    await flush();
    expect(tabPanelText()).toContain("const second = 2;");
  });
});

describe("the Files pencil", () => {
  it("offers no tab for a staged preview: a .png row has no pencil", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry("logo.png", "logo.png"));
    await renderWorkspace();
    await act(async () => {
      document.querySelector<HTMLElement>('[data-panel-tab="files"]')?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-tree-file")?.click();
    });
    await flush();
    expect(
      document.querySelector<HTMLButtonElement>('[aria-label="Open file in a tab"]'),
    ).toBeNull();
  });

  it("offers a tab for a read file: a .ts row has a pencil", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry("src/app.ts", "app.ts"));
    await renderWorkspace();
    await act(async () => {
      document.querySelector<HTMLElement>('[data-panel-tab="files"]')?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-tree-file")?.click();
    });
    await flush();
    expect(
      document.querySelector<HTMLButtonElement>('[aria-label="Open file in a tab"]'),
    ).not.toBeNull();
  });
});

describe("error detail ids", () => {
  it("a diff tab and the Changes panel name their own detail nodes", async () => {
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow("src/writer.ts"));
    // A daemon-shaped refusal: only it carries a detail, and only a detail
    // renders the hidden node this test counts.
    vi.mocked(workspaceGitDiff).mockRejectedValue({ code: "internal", message: "git exploded" });
    await renderWorkspace();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
    });
    await flush();
    await flush();

    const links = diffSurfaceErrorLinks().filter((link) => link.detailId !== null);
    expect(links).toHaveLength(2);
    const detailIds = links.map((link) => link.detailId);
    expect(new Set(detailIds).size).toBe(2);
    for (const link of links) {
      expect(link.describedBy).toBe(link.detailId);
    }
  });

  it("a file tab and the Files panel name their own detail nodes", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry("docs/SETUP.md", "SETUP.md"));
    vi.mocked(workspaceFileRead).mockRejectedValue({ code: "internal", message: "read exploded" });
    await renderWorkspace();
    await act(async () => {
      document.querySelector<HTMLElement>('[data-panel-tab="files"]')?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-tree-file")?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open file in a tab"]')?.click();
    });
    await flush();
    await flush();

    const links = diffSurfaceErrorLinks().filter((link) => link.detailId !== null);
    expect(links).toHaveLength(2);
    const detailIds = links.map((link) => link.detailId);
    expect(new Set(detailIds).size).toBe(2);
    for (const link of links) {
      expect(link.describedBy).toBe(link.detailId);
    }
  });
});
