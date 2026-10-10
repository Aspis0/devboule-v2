// @vitest-environment happy-dom

// `useEditableFile` directly: road selection, the missing-skip, stamp
// comparison, overlap guard, hidden pause, failed-read discipline and
// the BOM stash. The bridge is mocked; focus events drive polls so no
// test waits the 5 s interval.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkspaceEditableFile,
  WorkspaceFileVersion,
  WorkspaceFileWriteResult,
} from "../../../types/ipc";

vi.mock("../../../lib/tauri", () => ({
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
  appFileOpen,
  appFileVersion,
  remoteHostFileOpen,
  remoteHostFileVersion,
  workspaceFileEditorOpen,
  workspaceFileEditorVersion,
} from "../../../lib/tauri";
import { useEditableFile, type EditableFile } from "./useEditableFile";
import type { FileEditorModel } from "./model";
import { localWorkspaceKey, workspaceKey, type HostId, type WorkspaceKey } from "../hosts/hostIdentity";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "hook-subject";
const DEVICE = "device-9";

function ready(
  workspaceId: string,
  path: string,
  modifiedAt = 100,
): Extract<WorkspaceFileVersion, { status: "ready" }> {
  return { status: "ready", workspaceId, path, size: 4, modifiedAt, revision: `4:${modifiedAt}` };
}

function opened(
  content: string,
  version: WorkspaceFileVersion,
  hasBom = false,
): WorkspaceEditableFile {
  return {
    status: "ok",
    content,
    hasBom,
    version,
    size: content.length,
    error: null,
  };
}

describe("useEditableFile", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;
  let seen: EditableFile | null;

  const remoteKey = workspaceKey(DEVICE as HostId, WORKSPACE)!;
  const localKey: WorkspaceKey = localWorkspaceKey(WORKSPACE)!;

  function Harness({ workspaceKey, path }: { workspaceKey: WorkspaceKey; path: string }) {
    const result = useEditableFile(workspaceKey, path, 0);
    seen = result;
    return null;
  }

  async function render(workspaceKey: WorkspaceKey, path: string) {
    seen = null;
    if (root === undefined) root = createRoot(container);
    await act(async () => {
      root!.render(<Harness workspaceKey={workspaceKey} path={path} />);
    });
  }

  function model(): FileEditorModel {
    if (!seen?.model) throw new Error("the model did not open");
    return seen.model;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(workspaceFileEditorOpen).mockResolvedValue(opened("one\n", ready(WORKSPACE, "a.txt")));
    vi.mocked(workspaceFileEditorVersion).mockResolvedValue(ready(WORKSPACE, "a.txt"));
    vi.mocked(appFileOpen).mockResolvedValue(opened("hi\n", ready("", "/n")));
    vi.mocked(appFileVersion).mockResolvedValue(ready("", "/n"));
    vi.mocked(remoteHostFileOpen).mockResolvedValue(
      opened("remote\n", ready(WORKSPACE, "a.txt")),
    );
    vi.mocked(remoteHostFileVersion).mockResolvedValue(ready(WORKSPACE, "a.txt"));
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    container.remove();
    vi.clearAllMocks();
  });

  async function focus() {
    await act(async () => {
      window.dispatchEvent(new Event("focus"));
      await Promise.resolve();
    });
    // The poll's re-read is a second async hop.
    await act(async () => {
      await Promise.resolve();
    });
  }

  it("opens a workspace file on the workspace road", async () => {
    await render(localKey, "a.txt");

    expect(workspaceFileEditorOpen).toHaveBeenCalledWith(WORKSPACE, "a.txt");
    expect(model().getSnapshot()).toMatchObject({ status: "clean", content: "one\n" });
  });

  it("opens an outside path on the app road", async () => {
    await render(localKey, "/home/u/note.md");

    expect(appFileOpen).toHaveBeenCalledWith("/home/u/note.md");
    expect(workspaceFileEditorOpen).not.toHaveBeenCalled();
    expect(model().getSnapshot().content).toBe("hi\n");
  });

  it("opens a paired host's file over the relay", async () => {
    await render(remoteKey, "a.txt");

    expect(remoteHostFileOpen).toHaveBeenCalledWith(DEVICE, WORKSPACE, "a.txt");
    expect(model().getSnapshot().content).toBe("remote\n");
  });

  it("skips a missing poll of a never-created file", async () => {
    vi.mocked(workspaceFileEditorOpen).mockResolvedValueOnce(
      opened("", { status: "missing", workspaceId: WORKSPACE, path: "new.txt" }),
    );
    vi.mocked(workspaceFileEditorVersion).mockResolvedValue({
      status: "missing",
      workspaceId: WORKSPACE,
      path: "new.txt",
    });
    await render(localKey, "new.txt");

    await focus();

    // Still an empty clean editor — never a "Deleted" conflict over a
    // file that never existed.
    expect(model().getSnapshot()).toMatchObject({ status: "clean", content: "" });
  });

  it("adopts new bytes only when the stamp moved", async () => {
    await render(localKey, "a.txt");
    const opens = vi.mocked(workspaceFileEditorOpen).mock.calls.length;

    // Same stamp: version poll only, no re-read.
    await focus();
    expect(vi.mocked(workspaceFileEditorOpen).mock.calls.length).toBe(opens);

    // Moved stamp: re-reads and adopts while clean.
    vi.mocked(workspaceFileEditorVersion).mockResolvedValueOnce(ready(WORKSPACE, "a.txt", 102));
    vi.mocked(workspaceFileEditorOpen).mockResolvedValueOnce(
      opened("disk\n", ready(WORKSPACE, "a.txt", 102)),
    );
    await focus();
    expect(model().getSnapshot()).toMatchObject({ status: "clean", content: "disk\n" });
  });

  it("never overlaps polls", async () => {
    await render(localKey, "a.txt");
    let release!: (version: WorkspaceFileVersion) => void;
    vi.mocked(workspaceFileEditorVersion).mockImplementationOnce(
      () =>
        new Promise<WorkspaceFileVersion>((resolve) => {
          release = resolve;
        }),
    );
    const calls = vi.mocked(workspaceFileEditorVersion).mock.calls.length;

    // Two refreshes while one poll is in flight: the second is dropped.
    await act(async () => {
      seen!.refresh();
      seen!.refresh();
      await Promise.resolve();
    });
    expect(vi.mocked(workspaceFileEditorVersion).mock.calls.length).toBe(calls + 1);

    await act(async () => {
      release(ready(WORKSPACE, "a.txt"));
      await Promise.resolve();
    });
  });

  it("pauses while hidden", async () => {
    await render(localKey, "a.txt");
    const calls = vi.mocked(workspaceFileEditorVersion).mock.calls.length;

    Object.defineProperty(document, "hidden", { value: true, configurable: true });
    try {
      await focus();
    } finally {
      Object.defineProperty(document, "hidden", { value: false, configurable: true });
    }
    expect(vi.mocked(workspaceFileEditorVersion).mock.calls.length).toBe(calls);
  });

  it("feeds nothing when the re-read fails", async () => {
    await render(localKey, "a.txt");
    model().edit("local ");

    // Stamp moved but the bytes fail: no observation, no conflict — the
    // buffer stays dirty and the failure shows instead.
    vi.mocked(workspaceFileEditorVersion).mockResolvedValueOnce(ready(WORKSPACE, "a.txt", 102));
    vi.mocked(workspaceFileEditorOpen).mockRejectedValueOnce(new Error("gone"));
    await focus();

    expect(model().getSnapshot()).toMatchObject({ status: "dirty", content: "local " });
    expect(seen!.failure).toBe("The file could not be re-read.");
  });

  it("keeps the BOM flag across an adopted outside edit", async () => {
    vi.mocked(workspaceFileEditorOpen).mockResolvedValueOnce(
      opened("one\n", ready(WORKSPACE, "a.txt"), true),
    );
    const writes: string[] = [];
    const { workspaceFileEditorWrite } = await import("../../../lib/tauri");
    vi.mocked(workspaceFileEditorWrite).mockImplementation(async (_w, _p, content) => {
      writes.push(content);
      const result: WorkspaceFileWriteResult = {
        status: "written",
        modifiedAt: 101,
        size: content.length,
        revision: "x",
      };
      return result;
    });
    await render(localKey, "a.txt");

    // An outside edit the clean editor adopts, then a local keystroke and
    // a save: the BOM the file had comes back on the wire.
    vi.mocked(workspaceFileEditorVersion).mockResolvedValueOnce(ready(WORKSPACE, "a.txt", 102));
    vi.mocked(workspaceFileEditorOpen).mockResolvedValueOnce(
      opened("disk\n", ready(WORKSPACE, "a.txt", 102), true),
    );
    await focus();
    model().edit("disk\n!");
    await model().save();
    expect(writes.at(-1)?.charCodeAt(0)).toBe(0xfeff);
  });
});
