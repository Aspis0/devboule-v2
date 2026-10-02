// @vitest-environment happy-dom

// What the per-workspace tab memory learns, and how long it keeps it: it
// records the tab a workspace is actually SHOWING, whichever road chose it —
// the Overview, the roster reconcile — it outlives the surface, as does the
// workspace the user was standing in, a closed session leaves every
// workspace's memory, and a workspace that no longer exists takes its key
// with it.

import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  afterEachHarness,
  agentSession,
  beforeEachHarness,
  chipClick,
  clickDialogButton,
  clickMenuEntry,
  flush,
  liveSnapshot,
  plainClick,
  project,
  pushSnapshots,
  renderWorkspace,
  tabElement,
  tabTitles,
  terminalSession,
  unmountWorkspace,
} from "./bulkCloseHarness";
import {
  providersList,
  sessionsList,
  workspaceCreate,
  workspaceDelete,
  workspacesList,
} from "../../lib/tauri";
import { sharedSessionController } from "./workspaceSessions";
import type {
  ProviderInfo,
  Session,
  SessionStateSnapshot,
  Workspace as IpcWorkspace,
} from "../../types/ipc";

beforeEach(() => {
  beforeEachHarness();
});

afterEach(async () => {
  await afterEachHarness();
});

// Both worktrees, because that is the only isolation the daemon lets a
// workspace be created from or deleted from — the prune test needs a row
// that can go and a project with no local workspace to reuse.
const alpha: IpcWorkspace = {
  id: "workspace-1",
  projectId: project.id,
  title: "alpha",
  isolation: "worktree",
  path: "C:\\devboule-alpha",
};
const beta: IpcWorkspace = {
  id: "workspace-2",
  projectId: project.id,
  title: "beta",
  isolation: "worktree",
  path: "C:\\devboule-beta",
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

const designSnapshot = (): SessionStateSnapshot => ({
  ...liveSnapshot("design-agent", "Design agent", "acp"),
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

/** Only alpha holds tabs: the shape the live check found the bug in. A
 * workspace with no tab of its own leaves the global selection empty, so
 * nothing on the mount can name it — only the workspace in force can. */
function listedSessionsWithoutBetaTabs(): Session[] {
  return [terminalSession("a-one", "A one"), terminalSession("a-two", "A two")];
}

const claude: ProviderInfo = {
  id: "claude",
  executable: "claude",
  acpAvailable: true,
  authentication: "ok",
  protocol: "acp",
  origin: "user-binary",
  pickable: true,
};

async function showWorkspace(title: string): Promise<void> {
  const row = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")].find(
    (candidate) => candidate.textContent?.includes(title) === true,
  );
  if (row === undefined) throw new Error(`workspace row did not render: ${title}`);
  await act(async () => row.click());
  await flush();
}

async function openFromOverview(sessionId: string): Promise<void> {
  const trigger = document.querySelector<HTMLButtonElement>(".workspace-rate");
  if (trigger === null) throw new Error("the Overview trigger did not render");
  await act(async () => trigger.click());
  await flush();
  const option = document.querySelector<HTMLButtonElement>(`[data-overview-option="${sessionId}"]`);
  if (option === null) throw new Error(`the Overview did not offer ${sessionId}`);
  await act(async () => option.click());
  await flush();
}

/** Which workspace the sidebar has in force. */
function selectedWorkspaceRow(): string {
  const row = document.querySelector<HTMLButtonElement>(".workspace-row-selected");
  if (row === null) throw new Error("no workspace row is selected");
  return row.textContent ?? "";
}

async function deleteWorkspaceRow(index: number): Promise<void> {
  const row = [...document.querySelectorAll<HTMLButtonElement>(".workspace-row")][index];
  if (row === undefined) throw new Error(`workspace row ${index} did not render`);
  await act(async () => {
    row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
  });
  await clickMenuEntry("Delete workspace");
  await clickDialogButton("Delete");
  await act(async () => undefined);
  await act(async () => undefined);
}

/** The project's "+": one capable provider with no consent to ask for is
 * enough to reach the create. */
async function addWorkspaceFromProject(): Promise<void> {
  const add = document.querySelector<HTMLButtonElement>(
    `[aria-label="New workspace in ${project.name}"]`,
  );
  if (add === null) throw new Error("the project's + did not render");
  await act(async () => add.click());
  await flush();
  await flush();
}

describe("what the tab memory records", () => {
  it("records a session the Overview opened, not the chip it was left on", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await plainClick("a-two");
    await openFromOverview("a-one");
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("true");

    await showWorkspace("beta");
    await showWorkspace("alpha");
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("false");
  });

  it("records where the roster reconcile left the strip", async () => {
    // The unscoped Design agent leads the roster, so the reconcile that runs
    // when a-two leaves lands on it — not on A's first scoped session, which
    // is what a plain "first tab" answer would give.
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue([designAgent(), ...listedSessions()]);
    await renderWorkspace();

    await plainClick("a-two");
    await pushSnapshots([
      designSnapshot(),
      liveSnapshot("a-one", "A one"),
      liveSnapshot("b-one", "B one", "terminal"),
      liveSnapshot("b-two", "B two", "terminal"),
    ]);
    expect(tabElement("design-agent").getAttribute("aria-selected")).toBe("true");

    await showWorkspace("beta");
    await showWorkspace("alpha");
    expect(tabElement("design-agent").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("false");
  });

  it("keeps a closed session out of every workspace's memory", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue([...listedSessions(), designAgent()]);
    await renderWorkspace();

    // Clicked while beta was in force, so beta is the workspace that
    // remembers it.
    await showWorkspace("beta");
    await plainClick("design-agent");
    await showWorkspace("alpha");

    // Closed from ALPHA's strip. Its tab belongs to beta too, so beta's
    // memory has to forget it as well — and the same id coming back must
    // not turn that stale entry into a restore.
    await chipClick("design-agent");
    await flush();
    const design = sharedSessionController()
      .getState()
      .sessions.find((row) => row.id === "design-agent");
    if (design === undefined) throw new Error("the Design agent left the roster");
    await act(async () => sharedSessionController().open(design));

    await showWorkspace("beta");
    expect(tabElement("b-one").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("design-agent").getAttribute("aria-selected")).toBe("false");
  });
});

describe("how long the tab memory keeps it", () => {
  it("survives the surface being remounted", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await plainClick("a-two");
    await unmountWorkspace();
    // The session controller is app-lifetime too, so the remount finds the
    // same selection it left — which is what a surface change looks like.
    await renderWorkspace(false);
    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("true");

    await showWorkspace("beta");
    await showWorkspace("alpha");
    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("true");
  });

  it("comes back to the workspace in force, and to the tab it was left on", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue(listedSessionsWithoutBetaTabs());
    await renderWorkspace();

    await plainClick("a-two");
    await showWorkspace("beta");
    await unmountWorkspace();
    // What a surface change looks like: App keys the surface boundary by
    // surface, so Settings and back remounts this component. Nothing names
    // beta — its strip is empty, so the global selection is empty too, and
    // the project's first listed workspace is alpha.
    await renderWorkspace(false);
    expect(selectedWorkspaceRow()).toContain("beta");

    await showWorkspace("alpha");
    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("false");
  });

  it("keeps showing the remembered tab when the remount happens on its own workspace", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await plainClick("a-two");
    await unmountWorkspace();
    await renderWorkspace(false);

    expect(selectedWorkspaceRow()).toContain("alpha");
    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("true");
  });

  it("a selection naming another workspace does not carry the view off the one it came back to", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await showWorkspace("beta");
    await plainClick("b-two");
    await unmountWorkspace();
    await renderWorkspace(false);
    expect(selectedWorkspaceRow()).toContain("beta");

    // The global selection outlives the surface, and this one names a session
    // in another workspace: beta is where the user left off, so the view stays
    // and beta lands on the tab it was showing.
    await act(async () => sharedSessionController().select("a-two"));
    await flush();

    expect(selectedWorkspaceRow()).toContain("beta");
    expect(tabElement("b-two").getAttribute("aria-selected")).toBe("true");
    expect(tabTitles()).not.toContain("A two");
  });

  it("drops the key of a workspace that no longer exists", async () => {
    vi.mocked(workspacesList).mockResolvedValue([alpha, beta]);
    vi.mocked(sessionsList).mockResolvedValue(listedSessions());
    await renderWorkspace();

    await showWorkspace("beta");
    await plainClick("b-two");
    await showWorkspace("alpha");

    vi.mocked(workspacesList).mockResolvedValue([alpha]);
    await deleteWorkspaceRow(1);
    expect(workspaceDelete).toHaveBeenCalledWith("workspace-2");

    // A new workspace later takes the same id. A key that outlived the one
    // it belonged to would restore b-two here; a pruned one falls back to
    // the new workspace's first tab.
    vi.mocked(providersList).mockResolvedValue({ providers: [claude], unreadableDirs: 0 });
    vi.mocked(workspaceCreate).mockResolvedValue({ ...beta, path: "C:\\devboule-beta-2" });
    await addWorkspaceFromProject();
    expect(workspaceCreate).toHaveBeenCalledWith(project.id, "local");

    await showWorkspace("beta");
    expect(tabElement("b-one").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("b-two").getAttribute("aria-selected")).toBe("false");
  });
});
