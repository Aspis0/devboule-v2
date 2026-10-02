// @vitest-environment happy-dom

// Which tab a workspace lands on when the user comes back to it: the tab it
// was left on, whether that was a session or a tool pane, never a tab that
// was closed, and today's first-tab answer for a workspace the memory has
// never met.

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  agentSession,
  beforeEachHarness,
  chipClick,
  flush,
  plainClick,
  renderWorkspace,
  tabElement,
  terminalSession,
} from "./bulkCloseHarness";
import {
  sessionsList,
  workspaceGitDiff,
  workspaceGitStatus,
  workspacesList,
} from "../../lib/tauri";
import { toolTabId } from "./strip/toolTabs";
import type { Session, Workspace as IpcWorkspace } from "../../types/ipc";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

const alpha: IpcWorkspace = {
  id: "workspace-1",
  projectId: "project-1",
  title: "alpha",
  isolation: "local",
  path: "C:\\devboule",
};
const beta: IpcWorkspace = {
  id: "workspace-2",
  projectId: "project-1",
  title: "beta",
  isolation: "local",
  path: "C:\\side",
};

const inBeta = (id: string, title: string): Session => ({
  ...terminalSession(id, title),
  workspaceId: "workspace-2",
});

/** A Design agent: the surface creates it with no workspace at all. */
const designAgent = (): Session => ({
  ...agentSession("design-agent", "Design agent"),
  workspaceId: null,
});

function listedSessions(): Session[] {
  return [
    terminalSession("a-one", "A one"),
    terminalSession("a-two", "A two"),
    inBeta("b-one", "B one"),
    inBeta("b-two", "B two"),
  ];
}

function twoWorkspaces(): void {
  vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
}

async function showWorkspace(title: string): Promise<void> {
  const row = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
    (candidate) => candidate.textContent?.includes(title) === true,
  );
  if (row === undefined) throw new Error(`workspace row did not render: ${title}`);
  await act(async () => row.click());
  await flush();
}

function statusWithRow() {
  return {
    isGit: true,
    dirty: true,
    branch: "trunk",
    totals: { additions: 3, deletions: 1 },
    rows: [
      {
        path: "src/writer.ts",
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

/** The Changes panel's changed row, then its pencil: one Diff tab, open and
 * active in the workspace on screen. */
async function openActiveDiffTab(): Promise<string> {
  await act(async () => {
    document.querySelector<HTMLButtonElement>(".workspace-file-change")?.click();
  });
  await flush();
  await act(async () => {
    document.querySelector<HTMLButtonElement>('[aria-label="Open diff in a tab"]')?.click();
  });
  await flush();
  return toolTabId("diff", "workspace-1", "src/writer.ts");
}

function tabPanelText(): string {
  return document.querySelector("#workspace-panel-terminal")?.textContent ?? "";
}

function tabRendered(id: string): boolean {
  return document.querySelector(`#${CSS.escape(`workspace-session-tab-${id}`)}`) !== null;
}

describe("leaving a workspace and coming back", () => {
  it("restores the session tab the workspace was left on", async () => {
    twoWorkspaces();
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await plainClick("a-two");
    await showWorkspace("beta");
    expect(tabElement("b-one").getAttribute("aria-selected")).toBe("true");

    await showWorkspace("alpha");
    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("false");
  });

  it("restores the tool tab the workspace was left on: cached body at once, one fresh read", async () => {
    twoWorkspaces();
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    vi.mocked(workspaceGitStatus).mockResolvedValue(statusWithRow());
    vi.mocked(workspaceGitDiff).mockResolvedValue(diffReply("const first = 1;"));
    await renderWorkspace();

    const id = await openActiveDiffTab();
    expect(tabElement(id).getAttribute("aria-selected")).toBe("true");
    expect(tabPanelText()).toContain("const first = 1;");

    await showWorkspace("beta");
    // The tab belongs to alpha, so beta's strip cannot show it at all.
    expect(tabRendered(id)).toBe(false);
    expect(tabPanelText()).not.toContain("const first = 1;");

    // A diff may have moved while the tab was away, so the pane reads again on
    // every mount. The cache only decides that the old body stands until that
    // read lands: gate it, and anything on screen came from the cache.
    const before = vi.mocked(workspaceGitDiff).mock.calls.length;
    const gate = deferred<ReturnType<typeof diffReply>>();
    vi.mocked(workspaceGitDiff).mockReturnValue(gate.promise);
    await showWorkspace("alpha");

    expect(tabElement(id).getAttribute("aria-selected")).toBe("true");
    expect(tabPanelText()).toContain("const first = 1;");
    expect(tabPanelText()).not.toContain("Loading diff");
    expect(vi.mocked(workspaceGitDiff).mock.calls.length - before).toBe(1);
    gate.resolve(diffReply("const first = 1;"));
    await flush();
  });

  it("does not resurrect a tab closed before the round trip", async () => {
    twoWorkspaces();
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await plainClick("a-two");
    await chipClick("a-two");
    await flush();
    expect(tabRendered("a-two")).toBe(false);

    await showWorkspace("beta");
    await showWorkspace("alpha");
    expect(tabRendered("a-two")).toBe(false);
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("true");
  });

  it("a workspace the memory has never met still lands on its first tab", async () => {
    twoWorkspaces();
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await plainClick("a-two");
    await showWorkspace("beta");

    expect(tabElement("b-one").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("b-two").getAttribute("aria-selected")).toBe("false");
  });

  it("a session with no workspace stays in both strips, and is remembered where it was clicked", async () => {
    twoWorkspaces();
    vi.mocked(sessionsList).mockResolvedValue([...listedSessions(), designAgent()]);
    await renderWorkspace();

    await plainClick("design-agent");
    expect(tabElement("design-agent").getAttribute("aria-selected")).toBe("true");

    await showWorkspace("beta");
    expect(tabRendered("design-agent")).toBe(true);
    expect(tabElement("b-one").getAttribute("aria-selected")).toBe("true");

    await plainClick("design-agent");
    expect(tabElement("design-agent").getAttribute("aria-selected")).toBe("true");

    // Clicked while beta was in force, so beta is the workspace that
    // remembers it — and so is alpha, from its own click before the switch.
    await showWorkspace("alpha");
    expect(tabElement("design-agent").getAttribute("aria-selected")).toBe("true");
    await showWorkspace("beta");
    expect(tabElement("design-agent").getAttribute("aria-selected")).toBe("true");
  });
});
