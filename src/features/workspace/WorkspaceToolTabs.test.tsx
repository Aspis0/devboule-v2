// @vitest-environment happy-dom

// Tool tabs beside session tabs: opened from the right panel's pencil,
// focused and closed like any tab, rendered in the main pane. Rendered
// through Workspace so the registry-to-pane wiring is the thing proven.

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  agentSession,
  beforeEachHarness,
  bulkErrorBlock,
  chipClick,
  clickDialogButton,
  clickMenuEntry,
  dialog,
  flush,
  liveSnapshot,
  plainClick,
  pushSnapshots,
  renderWorkspace,
  resizeWindow,
  rightClick,
  settleCloseActs,
  tabElement,
  tabTitles,
  terminalSession,
} from "./bulkCloseHarness";
import {
  sessionStop,
  workspaceFilesList,
  workspaceFileRead,
  workspaceGitDiff,
  workspaceGitStatus,
} from "../../lib/tauri";
import { lookedAtSessionId } from "./presence";

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

/** The Changes pencil for one row: select the row, then take the pencil. */
async function openDiffPencil(path: string): Promise<void> {
  vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow(path));
  await renderWorkspace();
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
  });
  await flush();
  const pencil = document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]');
  if (pencil === null) throw new Error("diff pencil did not render");
  await act(async () => {
    pencil.click();
  });
  await flush();
}

/** The Files pencil: switch the panel, open the row, take the pencil. */
async function openFilePencil(path: string, name: string): Promise<void> {
  vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry(path, name));
  await renderWorkspace();
  await act(async () => {
    document.querySelector<HTMLElement>('[data-panel-tab="files"]')?.click();
  });
  await flush();
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".workspace-tree-file")?.click();
  });
  await flush();
  const pencil = document.querySelector<HTMLButtonElement>('[aria-label="Open file in a tab"]');
  if (pencil === null) throw new Error("file pencil did not render");
  await act(async () => {
    pencil.click();
  });
  await flush();
}

function toolTabButton(id: string): HTMLButtonElement {
  const tab = document.querySelector<HTMLButtonElement>(
    `#${CSS.escape(`workspace-session-tab-${id}`)}`,
  );
  if (tab === null) throw new Error(`tool tab did not render: ${id}`);
  return tab;
}

function toolTabGone(id: string): boolean {
  return document.querySelector(`#${CSS.escape(`workspace-session-tab-${id}`)}`) === null;
}

describe("opening from the panel", () => {
  it("the Changes pencil opens a diff tab, selects it and renders the diff", async () => {
    vi.mocked(workspaceGitDiff).mockResolvedValue({
      path: "src/writer.ts",
      isNew: false,
      isDeleted: false,
      additions: 1,
      deletions: 0,
      lines: [{ kind: "add", text: "const x = 1;" }],
      status: "ok",
      error: null,
    });
    await openDiffPencil("src/writer.ts");

    const tab = toolTabButton("tool:diff:workspace-1:src%2Fwriter.ts");
    expect(tab.getAttribute("aria-selected")).toBe("true");
    expect(tab.textContent).toContain("writer.ts");
    expect(tab.title).toBe("src/writer.ts");
    expect(document.body.textContent).toContain("const x = 1;");
    // The hidden session keeps its tab; the tool tab sits after the sessions.
    expect(tabTitles().length).toBe(4);
  });

  it("opening the same file again focuses the existing tab, never a duplicate", async () => {
    await openDiffPencil("src/writer.ts");
    await plainClick("agent-one");
    expect(
      toolTabButton("tool:diff:workspace-1:src%2Fwriter.ts").getAttribute("aria-selected"),
    ).toBe("false");
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
    });
    await flush();
    expect(
      document.querySelectorAll(
        '[id="workspace-session-tab-tool:diff:workspace-1:src%2Fwriter.ts"]',
      ),
    ).toHaveLength(1);
    expect(
      toolTabButton("tool:diff:workspace-1:src%2Fwriter.ts").getAttribute("aria-selected"),
    ).toBe("true");
  });

  it("the Files pencil opens a file tab with the preview", async () => {
    await openFilePencil("docs/SETUP.md", "SETUP.md");

    const tab = toolTabButton("tool:file:workspace-1:docs%2FSETUP.md");
    expect(tab.getAttribute("aria-selected")).toBe("true");
    expect(tab.textContent).toContain("SETUP.md");
    expect(vi.mocked(workspaceFileRead)).toHaveBeenCalledWith("workspace-1", "docs/SETUP.md");
    expect(document.body.textContent).toContain("preview bytes");
  });

  it("a tool chip carries the kind glyph instead of a status dot", async () => {
    await openDiffPencil("src/writer.ts");
    const tab = toolTabButton("tool:diff:workspace-1:src%2Fwriter.ts");
    expect(tab.querySelector(".workspace-status-dot")).toBeNull();
    expect(tab.querySelector('[data-mark="diff"]')).not.toBeNull();
  });
});

describe("closing a tool tab", () => {
  it("the chip closes locally: no confirm, no daemon call, successor left", async () => {
    vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry("docs/SETUP.md", "SETUP.md"));
    await openDiffPencil("src/writer.ts");
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
    const first = "tool:diff:workspace-1:src%2Fwriter.ts";
    const second = "tool:file:workspace-1:docs%2FSETUP.md";
    expect(toolTabButton(second).getAttribute("aria-selected")).toBe("true");

    // Middle tool tab closes onto its right neighbour.
    await chipClick(first);
    await settleCloseActs();
    expect(document.querySelector(DIALOG_SELECTOR_NULL)).toBeNull();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(toolTabGone(first)).toBe(true);
    expect(toolTabButton(second).getAttribute("aria-selected")).toBe("true");

    // Last tool tab closes onto its left neighbour.
    await chipClick(second);
    await settleCloseActs();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(tabElement("session-3").getAttribute("aria-selected")).toBe("true");
  });

  it("Delete on a focused tool chip closes it locally", async () => {
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    await act(async () => {
      const tab = toolTabButton(id);
      tab.focus();
      tab.dispatchEvent(new KeyboardEvent("keydown", { key: "Delete", bubbles: true }));
    });
    await settleCloseActs();
    expect(document.querySelector(DIALOG_SELECTOR_NULL)).toBeNull();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(toolTabGone(id)).toBe(true);
  });

  it("the chord walks onto the tool tab", async () => {
    await openDiffPencil("src/writer.ts");
    await plainClick("agent-one");
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    await flush();
    expect(tabElement("session-2").getAttribute("aria-selected")).toBe("true");
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    await flush();
    await act(async () => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: "]", altKey: true, shiftKey: true, bubbles: true }),
      );
    });
    await flush();
    expect(
      toolTabButton("tool:diff:workspace-1:src%2Fwriter.ts").getAttribute("aria-selected"),
    ).toBe("true");
  });
});

const DIALOG_SELECTOR_NULL = "[role='dialog'], [role='alertdialog']";

describe("mixed bulk close", () => {
  it("a failed mixed close puts the tool tabs back", async () => {
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    const before = tabTitles();
    expect(toolTabButton(id).getAttribute("aria-selected")).toBe("true");
    // Once-only, so the refusal cannot leak into the next test: the harness
    // clears calls between tests, never implementations.
    vi.mocked(sessionStop).mockRejectedValueOnce(new Error("daemon refused"));
    vi.mocked(sessionStop).mockRejectedValueOnce(new Error("daemon refused"));

    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    await settleCloseActs();
    await flush();
    await flush();

    // The sessions failed and say so; the tool tab left with the confirm
    // and came back at its index. The restore moves no selection: the
    // successor the close landed on keeps the pane.
    expect(tabTitles()).toEqual(before);
    expect(toolTabButton(id).getAttribute("aria-selected")).toBe("false");
    expect(tabElement("session-3").getAttribute("aria-selected")).toBe("true");
    expect(bulkErrorBlock().textContent).toContain("These closes didn't go through:");
  });

  it("a navigation after the confirm is never overridden", async () => {
    await openDiffPencil("src/writer.ts");
    const gates: Array<() => void> = [];
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          gates.push(() => resolve(undefined));
        }),
    );
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          gates.push(() => resolve(undefined));
        }),
    );

    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    // The mixed close already landed synchronously on session-3. Open a
    // file tab before the session acts settle.
    vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry("docs/SETUP.md", "SETUP.md"));
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
    const fileId = "tool:file:workspace-1:docs%2FSETUP.md";
    expect(toolTabButton(fileId).getAttribute("aria-selected")).toBe("true");

    await act(async () => {
      for (const resolve of gates) resolve();
    });
    await settleCloseActs();
    await flush();
    await flush();
    // The settle moves nothing: the file tab keeps the pane.
    expect(toolTabButton(fileId).getAttribute("aria-selected")).toBe("true");
    expect(document.querySelector("#workspace-panel-terminal")?.textContent).toContain(
      "preview bytes",
    );
  });

  it("focus never sits on the body during a confirm-close", async () => {
    await openDiffPencil("src/writer.ts");
    const gates: Array<() => void> = [];
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          gates.push(() => resolve(undefined));
        }),
    );
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          gates.push(() => resolve(undefined));
        }),
    );

    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    // The dialog unmounted on the click; the focus restore already ran in
    // the same tick — no window with the body holding focus.
    expect(document.activeElement).not.toBe(document.body);
    expect(document.activeElement?.id).toBe("workspace-session-tab-session-3");

    await act(async () => {
      for (const resolve of gates) resolve();
    });
    await settleCloseActs();
    await flush();
  });

  it("a restore after another tool tab keeps the new selection", async () => {
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    const rejecters: Array<(cause: unknown) => void> = [];
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejecters.push(reject);
        }),
    );
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejecters.push(reject);
        }),
    );

    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    // Before the refusal lands, open another tool tab: it takes the selection.
    vi.mocked(workspaceFilesList).mockResolvedValue(filesWithEntry("docs/SETUP.md", "SETUP.md"));
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
    const fileId = "tool:file:workspace-1:docs%2FSETUP.md";
    expect(toolTabButton(fileId).getAttribute("aria-selected")).toBe("true");

    await act(async () => {
      for (const reject of rejecters) reject(new Error("daemon refused"));
    });
    await settleCloseActs();
    await flush();
    await flush();
    // The failed close puts the diff tab back without moving selection.
    expect(toolTabGone(id)).toBe(false);
    expect(toolTabButton(fileId).getAttribute("aria-selected")).toBe("true");
    expect(toolTabButton(id).getAttribute("aria-selected")).toBe("false");
    expect(bulkErrorBlock().textContent).toContain("These closes didn't go through:");
  });

  it("a restore drops tabs whose workspace vanished during the act window", async () => {
    const { workspacesList, daemonStatus, sessionsList } = await import("../../lib/tauri");
    const main = {
      id: "workspace-1",
      projectId: "project-1",
      title: "main",
      isolation: "local" as const,
      path: "C:\\devboule",
    };
    const side = {
      id: "workspace-2",
      projectId: "project-1",
      title: "side",
      isolation: "local" as const,
      path: "C:\\side",
    };
    vi.mocked(workspacesList).mockResolvedValue([main, side]);
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    const rejecters: Array<(cause: unknown) => void> = [];
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejecters.push(reject);
        }),
    );
    vi.mocked(sessionStop).mockImplementationOnce(
      () =>
        new Promise<void>((_, reject) => {
          rejecters.push(reject);
        }),
    );

    await rightClick("session-3");
    await clickMenuEntry("Close other tabs");
    await clickDialogButton("Close");
    expect(toolTabGone(id)).toBe(true);

    // The daemon stops listing the tab's workspace while the acts are in
    // flight. The roster re-read fails, so the closing marks stand and the
    // refusals still land; the projects re-read succeeds, so the workspace
    // is gone when the restore runs.
    vi.mocked(workspacesList).mockResolvedValue([side]);
    vi.mocked(sessionsList).mockRejectedValue(new Error("daemon down"));
    vi.mocked(daemonStatus).mockRejectedValueOnce(new Error("daemon down"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await flush();

    await act(async () => {
      for (const reject of rejecters) reject(new Error("daemon refused"));
    });
    await settleCloseActs();
    await flush();
    await flush();
    // The session acts failed and say so.
    expect(bulkErrorBlock().textContent).toContain("These closes didn't go through:");

    // The orphan is state, not DOM: the view sits on the surviving workspace
    // now, which hides it. Re-list and go back — a resurrected tab for the
    // dropped workspace would render here.
    vi.mocked(workspacesList).mockResolvedValue([main, side]);
    vi.mocked(daemonStatus).mockRejectedValueOnce(new Error("daemon down"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await flush();
    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      rows.find((row) => row.textContent?.includes("main"))?.click();
    });
    await flush();
    expect(toolTabGone(id)).toBe(true);
  });

  it("the tool menu offers close entries only, and the confirm counts sessions", async () => {
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    await rightClick(id);
    const labels = [...document.querySelectorAll("[role='menuitem']")].map(
      (item) => item.textContent,
    );
    expect(labels).toEqual([
      "Close to the left",
      "Close to the right",
      "Close other tabs",
      "Close",
    ]);
    await resizeWindow();

    // "Close other tabs" from a session anchor covers the tool tab too:
    // the ask counts the sessions, and the tool rides along unnamed by archive.
    await rightClick("agent-one");
    await clickMenuEntry("Close other tabs");
    const confirm = dialog();
    expect(confirm.textContent).toContain("Close other tabs?");
    expect(confirm.textContent).toContain(
      "This will archive 2 terminal(s). The processes stop and every message stays in History.",
    );
    expect(confirm.textContent).toContain("1 tab closes too.");
    expect(confirm.textContent).not.toContain("archive 1 tab");

    await clickDialogButton("Close");
    await settleCloseActs();
    // The close lands in the same tick as the click; the flushes below only
    // settle the daemon promises the store already fired.
    await flush();
    await flush();
    await act(async () => {});
    expect(vi.mocked(sessionStop)).toHaveBeenCalledTimes(2);
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalledWith(id);
    expect(toolTabGone(id)).toBe(true);
    expect(tabTitles()).toHaveLength(1);
  });

  it("a tools-only selection closes at once, with no ask", async () => {
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    await act(async () => {
      toolTabButton(id).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    });
    await rightClick(id);
    await clickMenuEntry("Close");
    await settleCloseActs();
    expect(document.querySelector(DIALOG_SELECTOR_NULL)).toBeNull();
    expect(vi.mocked(sessionStop)).not.toHaveBeenCalled();
    expect(toolTabGone(id)).toBe(true);
    expect(tabTitles()).toHaveLength(3);
  });
});

describe("presence and scoping", () => {
  it("reports null while a tool tab is active, the session again after", async () => {
    await openDiffPencil("src/writer.ts");
    expect(lookedAtSessionId()).toBeNull();
    await plainClick("session-2");
    expect(lookedAtSessionId()).toBe("session-2");
  });

  it("switching workspace hides the other workspace's tool tabs", async () => {
    const { workspacesList } = await import("../../lib/tauri");
    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-1",
        projectId: "project-1",
        title: "main",
        isolation: "local",
        path: "C:\\devboule",
      },
      {
        id: "workspace-2",
        projectId: "project-1",
        title: "side",
        isolation: "local",
        path: "C:\\side",
      },
    ]);
    const { sessionsList } = await import("../../lib/tauri");
    vi.mocked(sessionsList).mockResolvedValue([
      agentSession("agent-one", "Agent one"),
      terminalSession("session-2", "shell two"),
      { ...terminalSession("other-1", "other one"), workspaceId: "workspace-2" },
    ]);
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    expect(toolTabButton(id)).toBeTruthy();

    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      const side = rows.find((row) => row.textContent?.includes("side"));
      if (side === undefined) throw new Error("workspace row did not render");
      side.click();
    });
    await flush();
    expect(toolTabGone(id)).toBe(true);
    expect(tabElement("other-1").getAttribute("aria-selected")).toBe("true");
  });

  it("a roster push never prunes a selected tool tab", async () => {
    await openDiffPencil("src/writer.ts");
    const id = "tool:diff:workspace-1:src%2Fwriter.ts";
    await act(async () => {
      toolTabButton(id).dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: true }));
    });
    await pushSnapshots([
      liveSnapshot("agent-one", "Agent one", "acp"),
      liveSnapshot("session-2", "shell two"),
      liveSnapshot("session-3", "shell three"),
    ]);
    await rightClick(id);
    const labels = [...document.querySelectorAll("[role='menuitem']")].map(
      (item) => item.textContent,
    );
    expect(labels).toContain("Close");
  });

  it("the same path in two workspaces is two tabs", async () => {
    const { workspacesList } = await import("../../lib/tauri");
    vi.mocked(workspacesList).mockResolvedValue([
      {
        id: "workspace-1",
        projectId: "project-1",
        title: "main",
        isolation: "local",
        path: "C:\\devboule",
      },
      {
        id: "workspace-2",
        projectId: "project-1",
        title: "side",
        isolation: "local",
        path: "C:\\side",
      },
    ]);
    await openDiffPencil("src/writer.ts");
    const first = "tool:diff:workspace-1:src%2Fwriter.ts";
    const second = "tool:diff:workspace-2:src%2Fwriter.ts";
    expect(first).not.toBe(second);

    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      const side = rows.find((row) => row.textContent?.includes("side"));
      if (side === undefined) throw new Error("workspace row did not render");
      side.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
    });
    await flush();
    expect(toolTabButton(second).getAttribute("aria-selected")).toBe("true");
    expect(toolTabGone(first)).toBe(true);

    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      const main = rows.find((row) => row.textContent?.includes("main"));
      if (main === undefined) throw new Error("workspace row did not render");
      main.click();
    });
    await flush();
    expect(toolTabGone(first)).toBe(false);
    expect(toolTabGone(second)).toBe(true);
  });

  it("a workspace that stops being listed drops its tool tabs", async () => {
    const { workspacesList, daemonStatus } = await import("../../lib/tauri");
    const main = {
      id: "workspace-1",
      projectId: "project-1",
      title: "main",
      isolation: "local" as const,
      path: "C:\\devboule",
    };
    const side = {
      id: "workspace-2",
      projectId: "project-1",
      title: "side",
      isolation: "local" as const,
      path: "C:\\side",
    };
    vi.mocked(workspacesList).mockResolvedValue([main, side]);
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow("src/writer.ts"));
    await renderWorkspace();
    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      rows.find((row) => row.textContent?.includes("side"))?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
    });
    await flush();
    const id = "tool:diff:workspace-2:src%2Fwriter.ts";
    expect(toolTabButton(id).getAttribute("aria-selected")).toBe("true");

    // The daemon stops listing the workspace; the reconnect re-reads the
    // projects, reconcile drops the workspace, and the prune takes its tabs.
    vi.mocked(workspacesList).mockResolvedValue([main]);
    vi.mocked(daemonStatus).mockRejectedValueOnce(new Error("daemon down"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await flush();
    expect(toolTabGone(id)).toBe(true);

    // Dropped, not hidden: re-listing the workspace does not bring the tab back.
    vi.mocked(workspacesList).mockResolvedValue([main, side]);
    vi.mocked(daemonStatus).mockRejectedValueOnce(new Error("daemon down"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await flush();
    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      rows.find((row) => row.textContent?.includes("side"))?.click();
    });
    await flush();
    expect(toolTabGone(id)).toBe(true);
  });

  it("a pruned workspace forgets what its tabs showed", async () => {
    const { workspacesList, daemonStatus } = await import("../../lib/tauri");
    const main = {
      id: "workspace-1",
      projectId: "project-1",
      title: "main",
      isolation: "local" as const,
      path: "C:\\devboule",
    };
    const side = {
      id: "workspace-2",
      projectId: "project-1",
      title: "side",
      isolation: "local" as const,
      path: "C:\\side",
    };
    vi.mocked(workspacesList).mockResolvedValue([main, side]);
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow("src/writer.ts"));
    vi.mocked(workspaceGitDiff).mockResolvedValue({
      path: "src/writer.ts",
      isNew: false,
      isDeleted: false,
      additions: 1,
      deletions: 0,
      lines: [{ kind: "add" as const, text: "const first = 1;" }],
      status: "ok" as const,
      error: null,
    });
    await renderWorkspace();
    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      rows.find((row) => row.textContent?.includes("side"))?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
    });
    await flush();
    const id = "tool:diff:workspace-2:src%2Fwriter.ts";
    expect(toolTabButton(id).getAttribute("aria-selected")).toBe("true");
    expect(document.body.textContent).toContain("const first = 1;");

    vi.mocked(workspacesList).mockResolvedValue([main]);
    vi.mocked(daemonStatus).mockRejectedValueOnce(new Error("daemon down"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await flush();
    expect(toolTabGone(id)).toBe(true);

    vi.mocked(workspacesList).mockResolvedValue([main, side]);
    vi.mocked(daemonStatus).mockRejectedValueOnce(new Error("daemon down"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(2000);
    });
    await flush();
    await flush();
    await act(async () => {
      const rows = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")];
      rows.find((row) => row.textContent?.includes("side"))?.click();
    });
    await flush();
    // The pruned tab left no seed: reopening re-reads from empty.
    let resolveRead!: (value: unknown) => void;
    vi.mocked(workspaceGitDiff).mockReturnValue(
      new Promise((resolve) => {
        resolveRead = resolve as (value: unknown) => void;
      }) as never,
    );
    await act(async () => {
      document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
    });
    await flush();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
    });
    await flush();
    expect(document.body.textContent).not.toContain("const first = 1;");
    resolveRead({
      path: "src/writer.ts",
      isNew: false,
      isDeleted: false,
      additions: 1,
      deletions: 0,
      lines: [{ kind: "add", text: "const second = 2;" }],
      status: "ok",
      error: null,
    });
    await flush();
    expect(document.body.textContent).toContain("const second = 2;");
  });
});
