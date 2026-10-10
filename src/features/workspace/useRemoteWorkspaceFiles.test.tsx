// @vitest-environment happy-dom

// Parallel folder expands: two folders loading at once must both land,
// whichever reply arrives first. A shared newest-wins counter would drop
// every reply but the last one's and leave the other folder loading
// forever.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { WorkspaceDirectory } from "../../types/ipc";

vi.mock("../../lib/tauri", () => ({
  remoteHostFilesList: vi.fn(),
}));

import { remoteHostFilesList } from "../../lib/tauri";
import { useRemoteWorkspaceFiles } from "./useRemoteWorkspaceFiles";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function listing(path: string, names: string[]): WorkspaceDirectory {
  return {
    path,
    entries: names.map((name) => ({
      path: path === "" ? name : `${path}/${name}`,
      name,
      kind: "dir" as const,
      size: null,
    })),
    capped: false,
    skipped: 0,
    error: null,
  };
}

describe("useRemoteWorkspaceFiles", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;
  let seen: ReturnType<typeof useRemoteWorkspaceFiles> | null = null;

  function Harness() {
    const result = useRemoteWorkspaceFiles("d", "w");
    seen = result;
    return null;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    container.remove();
    seen = null;
    vi.clearAllMocks();
  });

  it("lands every parallel expand whichever reply arrives first", async () => {
    const pending = new Map<string, (directory: WorkspaceDirectory) => void>();
    vi.mocked(remoteHostFilesList).mockImplementation(
      (_device: string, _workspace: string, path: string) => {
        if (path === "") return Promise.resolve(listing("", ["a", "b"]));
        return new Promise<WorkspaceDirectory>((resolve) => {
          pending.set(path, resolve);
        });
      },
    );
    if (root === undefined) root = createRoot(container);
    await act(async () => {
      root!.render(<Harness />);
    });
    // The root landed; both folders start loading together.
    await act(async () => {
      seen!.toggle("a");
    });
    await act(async () => {
      seen!.toggle("b");
    });

    // The second folder answers first: it must not cancel the first.
    await act(async () => {
      pending.get("b")!(listing("b", ["nested"]));
      await Promise.resolve();
    });
    await act(async () => {
      pending.get("a")!(listing("a", []));
      await Promise.resolve();
    });

    expect(seen!.cells["a"]?.reply).toEqual(listing("a", []));
    expect(seen!.cells["b"]?.reply).toEqual(listing("b", ["nested"]));
  });
});
