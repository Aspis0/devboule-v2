// @vitest-environment happy-dom

import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EditorTarget } from "../../lib/tauri";

vi.mock("../../lib/tauri", () => ({
  workspaceFileOpen: vi.fn(),
  editorTargetsList: vi.fn(),
}));

import { editorTargetsList, workspaceFileOpen } from "../../lib/tauri";
import { OpenInEditorAction } from "./OpenInEditorAction";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-open-action-subject";
const STORAGE_KEY = "devboule.preferredEditor";
const REMOTE_REFUSAL =
  "This workspace is on another device; its files cannot be opened by an editor on this computer.";
const LAUNCH_FAILED = "The editor could not be started.";

const TARGETS: EditorTarget[] = [
  { id: "cursor", label: "Cursor", kind: "editor" },
  { id: "vscode", label: "VS Code", kind: "editor" },
  { id: "file-manager", label: "Explorer", kind: "file_manager" },
];

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("OpenInEditorAction", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  beforeEach(() => {
    localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    vi.mocked(editorTargetsList).mockResolvedValue(TARGETS);
    vi.mocked(workspaceFileOpen).mockResolvedValue(undefined);
  });

  afterEach(async () => {
    if (root !== undefined) {
      await act(async () => root!.unmount());
      root = undefined;
    }
    container.remove();
    vi.clearAllMocks();
  });

  async function render(
    options: {
      path?: string;
      workspaceId?: string;
      line?: number;
      servedRemotely?: boolean;
      strict?: boolean;
    } = {},
  ) {
    if (root === undefined) root = createRoot(container);
    const action = (
      <OpenInEditorAction
        workspaceId={options.workspaceId ?? WORKSPACE}
        path={options.path ?? "docs/SETUP.md"}
        line={options.line}
        servedRemotely={options.servedRemotely}
      />
    );
    await act(async () => {
      root!.render(options.strict === true ? <StrictMode>{action}</StrictMode> : action);
    });
  }

  function pencil(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".open-in-editor-button");
    if (button === null) throw new Error("the pencil action did not render");
    return button;
  }

  function caret(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>(".open-in-editor-caret");
    if (button === null) throw new Error("the caret did not render");
    return button;
  }

  async function click(element: HTMLElement) {
    await act(async () => {
      element.click();
    });
  }

  function notice(): string | null {
    const alert = container.querySelector('[role="alert"]');
    // The sentence alone: ErrorText keeps the raw detail in a second,
    // visually hidden span beside it.
    return alert?.firstElementChild?.textContent ?? null;
  }

  function menuLabels(): string[] {
    return [...document.querySelectorAll('[role="menuitemradio"]')].map(
      (item) => item.textContent ?? "",
    );
  }

  it("names the pencil for assistive tech and as its tooltip", async () => {
    await render();
    expect(pencil().getAttribute("aria-label")).toBe("Open in editor");
    expect(pencil().getAttribute("title")).toBe("Open in editor");
  });

  it("opens the file with the first detected target when nothing is saved", async () => {
    await render();
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "docs/SETUP.md", undefined, "cursor");
  });

  it("passes the diff's line to the chosen editor", async () => {
    await render({ line: 42 });
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "docs/SETUP.md", 42, "cursor");
  });

  it("lists the detected targets in the caret menu and remembers the chosen one", async () => {
    await render();
    await click(caret());
    expect(menuLabels()).toEqual(["Cursor", "VS Code", "Explorer"]);
    const vscode = [...document.querySelectorAll('[role="menuitemradio"]')].find(
      (item) => item.textContent === "VS Code",
    );
    if (vscode === null || vscode === undefined) throw new Error("VS Code row did not render");
    await click(vscode as HTMLElement);

    expect(document.querySelector('[role="menu"]')).toBeNull();
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null")).toBe("vscode");
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "docs/SETUP.md", undefined, "vscode");
  });

  it("falls back to the first target when the saved id is gone", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify("zed"));
    await render();
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "docs/SETUP.md", undefined, "cursor");
  });

  it("writes a stale saved id back as the resolved fallback", async () => {
    localStorage.setItem(STORAGE_KEY, JSON.stringify("zed"));
    await render();
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null")).toBe("cursor");
  });

  it("keeps a newer id another window wrote before the stale repair ran", async () => {
    const detected = deferred<EditorTarget[]>();
    // StrictMode lists twice; both calls wait on the same detection.
    vi.mocked(editorTargetsList).mockReturnValue(detected.promise);
    localStorage.setItem(STORAGE_KEY, JSON.stringify("zed"));
    await render({ strict: true });

    await act(async () => {
      // The other window's write is in the store; its event is still queued
      // when the targets land and the stale "zed" gets repaired.
      localStorage.setItem(STORAGE_KEY, JSON.stringify("vscode"));
      detected.resolve(TARGETS);
    });
    await act(async () => {
      window.dispatchEvent(
        new StorageEvent("storage", { key: STORAGE_KEY, newValue: JSON.stringify("vscode") }),
      );
    });

    expect(JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "null")).toBe("vscode");
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "docs/SETUP.md", undefined, "vscode");
  });

  it("follows a preferred editor another window wrote", async () => {
    await render();
    await act(async () => {
      // The shared store already holds the other window's write when the
      // event arrives here — the dispatch stands in for that window.
      localStorage.setItem(STORAGE_KEY, JSON.stringify("vscode"));
      window.dispatchEvent(
        new StorageEvent("storage", {
          key: STORAGE_KEY,
          newValue: JSON.stringify("vscode"),
        }),
      );
    });
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "docs/SETUP.md", undefined, "vscode");
  });

  it("ignores a second click while the first open is pending", async () => {
    const first = deferred<void>();
    vi.mocked(workspaceFileOpen).mockReturnValueOnce(first.promise);
    await render();

    await click(pencil());
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledTimes(1);
    expect(pencil().disabled).toBe(true);

    await act(async () => {
      first.resolve(undefined);
    });
    expect(pencil().disabled).toBe(false);
    await click(pencil());
    expect(workspaceFileOpen).toHaveBeenCalledTimes(2);
  });

  it("drops a late failure from another file instead of landing it here", async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    vi.mocked(workspaceFileOpen).mockReturnValueOnce(first.promise);
    vi.mocked(workspaceFileOpen).mockReturnValueOnce(second.promise);

    await render({ path: "docs/a.md" });
    await click(pencil());
    await render({ path: "docs/b.md" });
    await click(pencil());

    await act(async () => {
      second.resolve(undefined);
    });
    expect(notice()).toBeNull();
    await act(async () => {
      first.reject({ code: "io", message: "the editor could not be started" });
    });
    expect(notice()).toBeNull();
  });

  it("keeps this file's failure when an older open succeeds late", async () => {
    const first = deferred<void>();
    const second = deferred<void>();
    vi.mocked(workspaceFileOpen).mockReturnValueOnce(first.promise);
    vi.mocked(workspaceFileOpen).mockReturnValueOnce(second.promise);

    await render({ path: "docs/a.md" });
    await click(pencil());
    await render({ path: "docs/b.md" });
    await click(pencil());

    await act(async () => {
      second.reject({ code: "io", message: "the editor could not be started" });
    });
    expect(notice()).toBe(LAUNCH_FAILED);
    await act(async () => {
      first.resolve(undefined);
    });
    expect(notice()).toBe(LAUNCH_FAILED);
  });

  it("clears the notice when the file or the workspace changes", async () => {
    vi.mocked(workspaceFileOpen).mockRejectedValueOnce({
      code: "io",
      message: "the editor could not be started",
    });
    await render({ path: "docs/a.md" });
    await click(pencil());
    expect(notice()).toBe(LAUNCH_FAILED);

    await render({ path: "docs/b.md" });
    expect(notice()).toBeNull();

    vi.mocked(workspaceFileOpen).mockRejectedValueOnce({
      code: "io",
      message: "the editor could not be started",
    });
    await render({ path: "docs/b.md", workspaceId: "other-workspace" });
    await click(pencil());
    expect(notice()).toBe(LAUNCH_FAILED);
    await render({ path: "docs/b.md", workspaceId: "third-workspace" });
    expect(notice()).toBeNull();
  });

  it("shows a failed launch with the production sentence", async () => {
    vi.mocked(workspaceFileOpen).mockRejectedValue({
      code: "io",
      message: "the editor could not be started",
    });
    await render();
    await click(pencil());
    expect(notice()).toBe(LAUNCH_FAILED);
  });

  it("disables the pencil with an honest tooltip when this machine has no target", async () => {
    vi.mocked(editorTargetsList).mockResolvedValue([]);
    await render();
    expect(pencil().disabled).toBe(true);
    expect(pencil().getAttribute("title")).toBe("No editor found on this computer");
    expect(caret().disabled).toBe(true);
    await click(pencil());
    expect(workspaceFileOpen).not.toHaveBeenCalled();
  });

  it("refuses a workspace served by another device before any open is attempted", async () => {
    await render({ servedRemotely: true });
    await click(pencil());
    expect(workspaceFileOpen).not.toHaveBeenCalled();
    expect(notice()).toBe(REMOTE_REFUSAL);
  });
});
