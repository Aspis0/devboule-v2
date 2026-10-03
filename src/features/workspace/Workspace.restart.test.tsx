// @vitest-environment happy-dom

// What a restart finds: the workspace the last run was standing in, and the
// tab that workspace was left on. Both are seeded the way the last run left
// them — the tab record and the open-tabs file — and read by a mount whose
// modules evaluate for the first time, which is what a page reload is.

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  afterEachHarness,
  beforeEachHarness,
  plainClick,
  restartWorkspace,
  tabElement,
  terminalSession,
} from "./bulkCloseHarness";
import { LOCAL_HOST_ID, localWorkspaceKey } from "./hosts/hostIdentity";
import { TAB_MEMORY_STORAGE_KEY } from "./tabMemoryStorage";
import type { Session, Workspace as IpcWorkspace } from "../../types/ipc";

// A restart builds a second copy of React, and the act the harness queues is
// the first copy's: React warns on every commit that this environment cannot
// support act, which says nothing true about what happens here.
(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

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
  isolation: "worktree",
  path: "C:\\devboule-alpha",
};
const beta: IpcWorkspace = {
  id: "workspace-2",
  projectId: "project-1",
  title: "beta",
  isolation: "worktree",
  path: "C:\\devboule-beta",
};

const alphaKey = localWorkspaceKey(alpha.id)!;
const betaKey = localWorkspaceKey(beta.id)!;

/** A's two terminals, the roster the restart finds. */
const alphaSessions = (): Session[] => [
  terminalSession("a-one", "A one"),
  terminalSession("a-two", "A two"),
];

/** What the tab memory held when the last run left. */
function lastRunRemembered(tabs: Record<string, string | null>): void {
  localStorage.setItem(TAB_MEMORY_STORAGE_KEY, JSON.stringify({ v: 1, tabs }));
}

/** The tabs the last run had open, with no selection among them: the file the
 * roster restore starts from, before anything has chosen a tab. */
function lastRunOpenTabs(sessions: readonly Session[]): void {
  const row = (session: Session) => ({
    id: session.id,
    workspaceId: session.workspaceId,
    createdAtMs: session.createdAtMs,
    hostId: LOCAL_HOST_ID,
    workspaceKey: alphaKey,
  });
  localStorage.setItem(
    "devboule.openSessionTabs",
    JSON.stringify({ version: 2, tabs: sessions.map(row), selected: null }),
  );
}

/** The workspace the last run was standing in. */
function lastRunStoodIn(workspaceKey: string): void {
  localStorage.setItem("devboule.lastWorkspace", workspaceKey);
}

describe("what a restart finds", () => {
  it("opens the workspace on the tab the last run left it on", async () => {
    lastRunRemembered({ [alphaKey]: "a-two", [betaKey]: null });
    lastRunOpenTabs(alphaSessions());

    await restartWorkspace(alphaSessions(), [alpha, beta]);

    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("false");
    // The restore selects what the memory says, so what the last run left is
    // still what storage holds — the selection made on the way in never became
    // a new answer of its own.
    expect(JSON.parse(localStorage.getItem(TAB_MEMORY_STORAGE_KEY) ?? "null")).toEqual({
      v: 1,
      tabs: { [alphaKey]: "a-two", [betaKey]: null },
    });
  });

  it("stands in the workspace the last run was standing in", async () => {
    lastRunRemembered({ [alphaKey]: null, [betaKey]: null });
    lastRunOpenTabs(alphaSessions());
    lastRunStoodIn(betaKey);

    await restartWorkspace(alphaSessions(), [alpha, beta]);

    const row = document.querySelector(".workspace-row-selected");
    expect(row?.textContent).toContain("beta");
  });

  it("remembers what the user picks once the restart is over", async () => {
    lastRunRemembered({ [alphaKey]: "a-two", [betaKey]: null });
    lastRunOpenTabs(alphaSessions());
    await restartWorkspace(alphaSessions(), [alpha, beta]);

    await plainClick("a-one");
    await restartWorkspace(alphaSessions(), [alpha, beta]);

    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("true");
    expect(tabElement("a-two").getAttribute("aria-selected")).toBe("false");
  });

  it("leaves a restored strip standing even where the memory says empty", async () => {
    lastRunRemembered({ [alphaKey]: null, [betaKey]: null });
    lastRunOpenTabs(alphaSessions());

    await restartWorkspace(alphaSessions(), [alpha, beta]);

    // A workspace left empty is the answer to a navigation into it, never to
    // a restart: the tabs the roster brought back still stand where they are.
    expect(tabElement("a-one").getAttribute("aria-selected")).toBe("true");
    expect(JSON.parse(localStorage.getItem(TAB_MEMORY_STORAGE_KEY) ?? "null")).toEqual({
      v: 1,
      tabs: { [alphaKey]: null, [betaKey]: null },
    });
  });
});
