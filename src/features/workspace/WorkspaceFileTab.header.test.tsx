// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceEditableFile, WorkspaceFileContent } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFileRead: vi.fn(),
  workspaceFilePreviewStage: vi.fn(),
  workspaceFilePreviewUnstage: vi.fn(),
  workspaceFileOpen: vi.fn(),
  editorTargetsList: vi.fn(),
  workspaceFileEditorOpen: vi.fn(),
  workspaceFileEditorVersion: vi.fn(),
  workspaceFileEditorWrite: vi.fn(),
  appFileOpen: vi.fn(),
  appFileVersion: vi.fn(),
  appFileWrite: vi.fn(),
  remoteHostFileOpen: vi.fn(),
  remoteHostFileVersion: vi.fn(),
  remoteHostFileWrite: vi.fn(),
}));

import {
  editorTargetsList,
  workspaceFileEditorOpen,
  workspaceFileOpen,
  workspaceFileRead,
} from "../../lib/tauri";
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

function editableFile(overrides: Partial<WorkspaceEditableFile> = {}): WorkspaceEditableFile {
  return {
    status: "ok",
    content: "hello\n",
    hasBom: false,
    version: {
      status: "ready",
      workspaceId: WORKSPACE,
      path: "a.txt",
      size: 6,
      modifiedAt: 1_758_000_000_000,
      revision: "6:1758000000000",
    },
    size: 6,
    error: null,
    ...overrides,
  };
}

describe("WorkspaceFileTab header", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  beforeEach(() => {
    resetFileTabModeForTests();
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceFileRead).mockResolvedValue(content());
    vi.mocked(workspaceFileEditorOpen).mockResolvedValue(editableFile({ content: "hello\n" }));
    vi.mocked(workspaceFileOpen).mockResolvedValue(undefined);
    vi.mocked(editorTargetsList).mockResolvedValue([
      { id: "cursor", label: "Cursor", kind: "editor" },
    ]);
    localStorage.clear();
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    container.remove();
    vi.clearAllMocks();
  });

  async function renderTab(path: string, key = "a") {
    const mount = (
      <WorkspaceFileTab
        key={key}
        workspaceKey={keyFor(WORKSPACE)}
        path={path}
        refreshNonce={0}
        cache={new Map()}
      />
    );
    if (root === undefined) root = createRoot(container);
    await act(async () => {
      root!.render(mount);
    });
  }

  function meta(): string {
    const match = container.querySelector(".workspace-file-tab-meta");
    if (match === null) throw new Error("header meta did not render");
    return match.textContent ?? "";
  }

  function segButtons(): HTMLButtonElement[] {
    return Array.from(
      container.querySelectorAll<HTMLButtonElement>(".workspace-file-tab-seg-button"),
    );
  }

  it("shows basename, parent directory and size", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(content({ size: 2150, hasMore: true }));
    await renderTab("docs/SETUP.md");

    const name = container.querySelector(".workspace-file-tab-name");
    expect(name?.textContent).toBe("SETUP.md");
    expect(meta()).toBe("docs · 2.1 KB");
  });

  it("shows the line count only for a fully loaded file", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ size: 2150, lines: 48, hasMore: false, truncated: false }),
    );
    await renderTab("docs/SETUP.md");

    expect(meta()).toBe("docs · 2.1 KB · 48 lines");
  });

  it("never shows a window count as the file's count", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(
      content({ size: 2150, fromLine: 41, lines: 48, hasMore: false, truncated: false }),
    );
    await renderTab("docs/SETUP.md");

    expect(meta()).toBe("docs · 2.1 KB");
  });

  it("leaves the line count out while another window follows or one was cut", async () => {
    vi.mocked(workspaceFileRead)
      .mockResolvedValueOnce(content({ size: 2150, lines: 48, hasMore: true }))
      .mockResolvedValueOnce(
        content({ size: 2150, lines: 1, hasMore: false, truncated: true, note: "cut" }),
      );
    await renderTab("docs/SETUP.md");
    expect(meta()).toBe("docs · 2.1 KB");

    await renderTab("docs/SETUP.md", "b");
    expect(meta()).toBe("docs · 2.1 KB");
  });

  it("omits the directory part of a root-level file", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(content({ size: 2150, lines: 48 }));
    await renderTab("README.md");

    expect(meta()).toBe("2.1 KB · 48 lines");
  });

  it("splits a Windows-separator path for the name and the directory", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue(content({ size: 6 }));
    await renderTab("docs\\file.md");

    expect(container.querySelector(".workspace-file-tab-name")?.textContent).toBe("file.md");
    expect(meta()).toBe("docs · 6 B · 1 line");
  });

  it("edits a non-Markdown file with no mode control", async () => {
    await renderTab("src/main.rs");

    expect(segButtons()).toHaveLength(0);
    expect(container.querySelector('[data-testid="file-source-editor"]')).not.toBeNull();
  });

  it("recognises .markdown and uppercase extensions as Markdown", async () => {
    await renderTab("docs/README.MARKDOWN");

    expect(segButtons()).toHaveLength(2);
  });

  it("defaults a Markdown file to Preview", async () => {
    await renderTab("docs/SETUP.md");

    expect(container.querySelector(".workspace-file-tab-preview")).not.toBeNull();
    expect(container.querySelector(".workspace-file-tab-source")).toBeNull();
  });

  it("switches to Source when its segment is clicked", async () => {
    await renderTab("docs/SETUP.md");
    const buttons = segButtons();
    if (buttons[1] === undefined) throw new Error("Source segment did not render");

    await act(async () => {
      buttons[1].click();
    });

    expect(container.querySelector('[data-testid="file-source-editor"]')).not.toBeNull();
    expect(container.querySelector(".workspace-file-tab-preview")).toBeNull();
  });

  it("offers Open in editor with the tab's workspace id and path", async () => {
    await renderTab("docs/SETUP.md");
    const button = container.querySelector<HTMLButtonElement>(".open-in-editor-button");
    if (button === null) throw new Error("the pencil action did not render");
    expect(button.getAttribute("aria-label")).toBe("Open in editor");
    expect(button.getAttribute("title")).toBe("Open in editor");

    await act(async () => {
      button.click();
    });

    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "docs/SETUP.md", undefined, "cursor");
  });

  it("remembers the mode across tabs for the app run", async () => {
    await renderTab("docs/a.md");
    const buttons = segButtons();
    if (buttons[1] === undefined) throw new Error("Source segment did not render");
    await act(async () => {
      buttons[1].click();
    });

    // A different path, a fresh mount: the module's memory survives it.
    await renderTab("docs/b.md", "second");

    expect(container.querySelector(".workspace-file-tab-preview")).toBeNull();
    expect(container.querySelector('[data-testid="file-source-editor"]')).not.toBeNull();
  });
});
