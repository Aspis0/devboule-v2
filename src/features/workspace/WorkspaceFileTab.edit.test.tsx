// @vitest-environment happy-dom

// The File tab as an editor: a text file opens in CodeMirror, typing
// marks it dirty, the debounced save writes through the bridge, and an
// outside change while dirty raises the conflict banner with
// Overwrite/Reload. The bridge is mocked; typing goes through the real
// CodeMirror view so the dirty/save/conflict roads are the shipped ones,
// not a model double.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceEditableFile, WorkspaceFileWriteResult } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  workspaceFileRead: vi.fn(),
  workspaceFilePreviewStage: vi.fn(),
  workspaceFilePreviewUnstage: vi.fn(),
  editorTargetsList: vi.fn(async () => []),
  workspaceFileEditorOpen: vi.fn(),
  workspaceFileEditorVersion: vi.fn(),
  workspaceFileEditorWrite: vi.fn(),
  appFileOpen: vi.fn(),
  appFileVersion: vi.fn(),
  appFileWrite: vi.fn(),
  remoteHostFileOpen: vi.fn(),
  remoteHostFileVersion: vi.fn(),
  remoteHostFileWrite: vi.fn(),
  daemonStatus: vi.fn(),
}));

import {
  appFileOpen,
  appFileWrite,
  daemonStatus,
  workspaceFileEditorOpen,
  workspaceFileEditorVersion,
  workspaceFileEditorWrite,
  workspaceFileRead,
} from "../../lib/tauri";
import { EditorView } from "@codemirror/view";
import { WorkspaceFileTab } from "./WorkspaceFileTab";

import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-file-tab-edit-subject";

function windowed(content = "one\n"): Record<string, unknown> {
  return {
    status: "ok",
    kind: "text",
    content,
    size: content.length,
    modifiedAt: 100,
    error: null,
    fromLine: 1,
    lines: 1,
    hasMore: false,
    truncated: false,
    note: null,
  };
}

function opened(
  content: string,
  version: WorkspaceEditableFile["version"] = {
    status: "ready",
    workspaceId: WORKSPACE,
    path: "a.txt",
    size: content.length,
    modifiedAt: 100,
    revision: "4:100",
  },
): WorkspaceEditableFile {
  return { status: "ok", content, hasBom: false, version, size: content.length, error: null };
}

function written(modifiedAt = 101): WorkspaceFileWriteResult {
  return { status: "written", modifiedAt, size: 4, revision: `4:${modifiedAt}` };
}

describe("WorkspaceFileTab editing", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceFileRead).mockResolvedValue(windowed() as never);
    vi.mocked(workspaceFileEditorOpen).mockResolvedValue(opened("one\n"));
    vi.mocked(daemonStatus).mockResolvedValue({
      state: "connected",
      pid: 1,
      instanceId: "1",
      protocolVersion: 34,
      clients: 1,
      capabilities: [],
      message: null,
    } as never);
    vi.mocked(workspaceFileEditorVersion).mockResolvedValue({
      status: "ready",
      workspaceId: WORKSPACE,
      path: "a.txt",
      size: 4,
      modifiedAt: 100,
      revision: "4:100",
    });
    vi.mocked(workspaceFileEditorWrite).mockResolvedValue(written());
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    container.remove();
    vi.clearAllMocks();
  });

  async function renderTab(path = "a.txt", refreshNonce = 0) {
    if (root === undefined) root = createRoot(container);
    await act(async () => {
      root!.render(
        <WorkspaceFileTab
          workspaceKey={keyFor(WORKSPACE)}
          path={path}
          refreshNonce={refreshNonce}
          cache={new Map()}
        />,
      );
    });
  }

  function editor(): HTMLElement {
    const found = container.querySelector<HTMLElement>('[data-testid="file-source-editor"]');
    if (!found) throw new Error("the CodeMirror editor did not mount");
    return found;
  }

  /** Type through the real CodeMirror view: a genuine transaction, so
   * the view-to-model road under test is the shipped one. (happy-dom has
   * no editing engine, so synthetic `beforeinput` never lands.) */
  async function type(text: string): Promise<void> {
    const view = EditorView.findFromDOM(editor());
    if (!view) throw new Error("no CodeMirror view on the editor host");
    await act(async () => {
      view.dispatch({ changes: { from: 0, insert: text } });
    });
  }

  /** Save now through the view's own keymap, like Ctrl/Cmd+S does. */
  async function saveNow(): Promise<void> {
    const content = editor().querySelector<HTMLElement>(".cm-content");
    if (!content) throw new Error("the CodeMirror content did not mount");
    await act(async () => {
      content.dispatchEvent(
        new KeyboardEvent("keydown", { key: "s", ctrlKey: true, bubbles: true, cancelable: true }),
      );
    });
    // Let the mocked write resolve.
    await act(async () => {
      await Promise.resolve();
    });
  }

  function dirtyDot(): HTMLElement | null {
    return container.querySelector<HTMLElement>(".file-editor-dirty");
  }

  function conflictAlert(): HTMLElement | null {
    return container.querySelector<HTMLElement>('[data-testid="file-conflict-alert"]');
  }

  it("opens a text file in the editor", async () => {
    await renderTab();

    expect(editor()).not.toBeNull();
    expect(workspaceFileEditorOpen).toHaveBeenCalledWith(WORKSPACE, "a.txt");
    expect(container.querySelector('[data-testid="file-editor-bar"]')).not.toBeNull();
  });

  it("marks dirty on typing and saves through the bridge", async () => {
    await renderTab();
    expect(dirtyDot()).toBeNull();

    await type("two ");
    expect(dirtyDot()).not.toBeNull();

    await saveNow();

    expect(workspaceFileEditorWrite).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceFileEditorWrite).mock.calls[0]?.[2]).toContain("two ");
    expect(dirtyDot()).toBeNull();
  });

  it("raises the conflict banner when the disk moves under a dirty buffer", async () => {
    await renderTab();
    await type("local ");

    // An outside edit lands: the next poll re-reads new bytes.
    vi.mocked(workspaceFileEditorOpen).mockResolvedValueOnce(opened("disk\n"));
    vi.mocked(workspaceFileEditorVersion).mockResolvedValueOnce({
      status: "ready",
      workspaceId: WORKSPACE,
      path: "a.txt",
      size: 5,
      modifiedAt: 102,
      revision: "5:102",
    });
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      await Promise.resolve();
    });
    // The poll's re-read is a second async hop.
    await act(async () => {
      await Promise.resolve();
    });

    const alert = conflictAlert();
    expect(alert).not.toBeNull();
    expect(alert?.textContent).toContain("Changed on disk");
    expect(
      Array.from(alert?.querySelectorAll("button") ?? []).map((button) => button.textContent),
    ).toEqual(["Overwrite", "Reload"]);
    // The local buffer is preserved under the banner.
    expect(workspaceFileEditorWrite).not.toHaveBeenCalled();
  });

  it("reloads the disk contents from the banner and goes clean", async () => {
    await renderTab();
    await type("local ");
    vi.mocked(workspaceFileEditorOpen).mockResolvedValueOnce(opened("disk\n"));
    vi.mocked(workspaceFileEditorVersion).mockResolvedValueOnce({
      status: "ready",
      workspaceId: WORKSPACE,
      path: "a.txt",
      size: 5,
      modifiedAt: 102,
      revision: "5:102",
    });
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      await Promise.resolve();
    });
    await act(async () => {
      await Promise.resolve();
    });
    expect(conflictAlert()).not.toBeNull();

    const reload = Array.from(conflictAlert()?.querySelectorAll("button") ?? []).find(
      (button) => button.textContent === "Reload",
    );
    if (!reload) throw new Error("Reload did not render");
    // Local edits exist, so the banner asks through the confirm dialog.
    await act(async () => {
      reload.click();
    });
    const confirm = Array.from(document.querySelectorAll("button")).find(
      (button) =>
        button.textContent === "Reload" &&
        button.closest('[data-testid="file-conflict-alert"]') === null,
    );
    if (!confirm) throw new Error("the reload confirm did not ask");
    await act(async () => {
      confirm.click();
    });

    expect(conflictAlert()).toBeNull();
    expect(dirtyDot()).toBeNull();
  });

  it("saves a missing file by creating it", async () => {
    vi.mocked(workspaceFileRead).mockResolvedValue({
      ...windowed(),
      status: "refused",
      kind: null,
      content: null,
      size: null,
      modifiedAt: null,
      error: "the requested path does not exist",
    } as never);
    vi.mocked(workspaceFileEditorOpen).mockResolvedValue(
      opened("", { status: "missing", workspaceId: WORKSPACE, path: "new.txt" }),
    );
    await renderTab("new.txt");

    await type("born ");
    await saveNow();

    expect(workspaceFileEditorWrite).toHaveBeenCalledTimes(1);
    // A create names no expected version but an explicit intent.
    expect(vi.mocked(workspaceFileEditorWrite).mock.calls[0]?.slice(3)).toEqual([null, null, true]);
  });

  it("stays read-only with a note against a stale daemon", async () => {
    vi.mocked(daemonStatus).mockResolvedValue({
      state: "connected",
      pid: 1,
      instanceId: "1",
      protocolVersion: 32,
      clients: 1,
      capabilities: [],
      message: null,
    } as never);
    await renderTab();

    // The editor frames would kill a v32 connection, so they never
    // leave: no editor, a short note, and the windowed body instead.
    expect(workspaceFileEditorOpen).not.toHaveBeenCalled();
    expect(container.querySelector('[data-testid="file-source-editor"]')).toBeNull();
    expect(container.textContent).toContain("Update the daemon to edit files.");
  });

  it("keeps the buffer across a re-click instead of re-opening", async () => {
    await renderTab();
    await type("unsaved ");
    expect(dirtyDot()).not.toBeNull();
    const opens = vi.mocked(workspaceFileEditorOpen).mock.calls.length;

    // Re-clicking the active tab polls; it must not rebuild the model.
    await renderTab("a.txt", 1);
    await act(async () => {
      await Promise.resolve();
    });

    expect(vi.mocked(workspaceFileEditorOpen).mock.calls.length).toBe(opens);
    expect(dirtyDot()).not.toBeNull();
  });

  it("flushes pending text on unmount instead of dropping it", async () => {
    await renderTab();
    await type("unsaved ");
    expect(workspaceFileEditorWrite).not.toHaveBeenCalled();

    // A tab switch unmounts the pane: the buffer saves now instead of
    // dying with the debounce timer.
    await act(async () => {
      root!.unmount();
      root = undefined;
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(workspaceFileEditorWrite).toHaveBeenCalledTimes(1);
    expect(vi.mocked(workspaceFileEditorWrite).mock.calls[0]?.[2]).toContain("unsaved ");
  });

  it("flushes and asks on window close with unsaved text", async () => {
    await renderTab();
    await type("unsaved ");

    const event = new Event("beforeunload", { cancelable: true });
    await act(async () => {
      window.dispatchEvent(event);
    });
    await act(async () => {
      await Promise.resolve();
    });

    expect(workspaceFileEditorWrite).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
  });

  it("edits an outside file on the app road", async () => {
    vi.mocked(appFileOpen).mockResolvedValue({
      status: "ok",
      content: "hi\n",
      hasBom: false,
      version: {
        status: "ready",
        workspaceId: "",
        path: "/home/u/note.txt",
        size: 3,
        modifiedAt: 100,
        revision: "3:100",
      },
      size: 3,
      error: null,
    });
    vi.mocked(appFileWrite).mockResolvedValue({
      status: "written",
      modifiedAt: 101,
      size: 4,
      revision: "4:101",
    });
    // The windowed read refuses the outside spelling; the editor then
    // tries the workspace road, which refuses it too — and only then
    // the app road opens the real file. One hop each, never a loop.
    vi.mocked(workspaceFileRead).mockResolvedValueOnce({
      status: "refused",
      kind: null,
      content: null,
      size: null,
      modifiedAt: null,
      error: "the requested path is outside the workspace folder",
    } as never);
    vi.mocked(workspaceFileEditorOpen).mockResolvedValueOnce({
      status: "refused",
      kind: null,
      content: null,
      size: null,
      version: null,
      error: "the requested path is outside the workspace folder",
    } as never);
    await renderTab("/home/u/note.txt");

    expect(appFileOpen).toHaveBeenCalledWith("/home/u/note.txt");
    expect(workspaceFileEditorOpen).toHaveBeenCalledWith(WORKSPACE, "/home/u/note.txt");
    expect(container.querySelector('[data-testid="file-source-editor"]')).not.toBeNull();

    await type("hey ");
    await saveNow();

    expect(appFileWrite).toHaveBeenCalledTimes(1);
    expect(vi.mocked(appFileWrite).mock.calls[0]?.[0]).toBe("/home/u/note.txt");
  });

  it("opens a subdirectory link by its absolute spelling", async () => {
    // The finding's case end to end: the transcript resolved `lib/a.ts`
    // against the session cwd `<root>/sub` and opened the absolute
    // spelling; the daemon maps it inside the workspace (pinned
    // daemon-side), so the tab sends the spelling it was given.
    await renderTab("/repo/ws/sub/a.txt");

    expect(workspaceFileEditorOpen).toHaveBeenCalledWith(WORKSPACE, "/repo/ws/sub/a.txt");
    expect(appFileOpen).not.toHaveBeenCalled();
    expect(container.querySelector('[data-testid="file-source-editor"]')).not.toBeNull();
  });
});
