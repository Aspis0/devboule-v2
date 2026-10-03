// The tool-tab model: the id namespace, open/dedupe, labels, strip order,
// the shared successor rule and workspace pruning.

import { describe, expect, it } from "vitest";
import {
  composeStripTabs,
  isToolTabId,
  makeBrowserTab,
  makeToolTab,
  openToolTabs,
  pruneToolTabsForWorkspaces,
  successorOf,
  toolTabId,
  toolTabLabel,
  type ToolTab,
} from "./toolTabs";
import type { Session } from "../../../types/ipc";
import { LOCAL_HOST_ID, workspaceKey, type HostId, type WorkspaceKey } from "../hosts/hostIdentity";

/** A host that owns a workspace with the same id as the local one: no code
 * path can produce one yet, so the tests name it themselves. */
const PEER = "peer-7" as HostId;

function at(workspaceId: string, hostId: HostId = LOCAL_HOST_ID): WorkspaceKey {
  return workspaceKey(hostId, workspaceId)!;
}

function session(id: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal",
    title: id,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

describe("toolTabId", () => {
  it("namespaces kinds, workspaces and paths apart", () => {
    expect(toolTabId("diff", "ws", "a.ts")).toBe("tool:diff:ws:a.ts");
    expect(toolTabId("file", "ws", "a.ts")).toBe("tool:file:ws:a.ts");
    expect(toolTabId("diff", "ws", "a.ts")).not.toBe(toolTabId("diff", "ws", "b.ts"));
    expect(toolTabId("diff", "ws-1", "a.ts")).not.toBe(toolTabId("diff", "ws-2", "a.ts"));
  });

  it("encodes each part before joining, so separators cannot collide", () => {
    expect(toolTabId("diff", "ws:a", "b")).not.toBe(toolTabId("diff", "ws", "a:b"));
    expect(toolTabId("diff", "ws", "a/b")).toBe("tool:diff:ws:a%2Fb");
    expect(toolTabId("diff", "ws", "a/b")).not.toBe(toolTabId("diff", "ws", "a%2Fb"));
  });

  it("never collides with a session id", () => {
    expect(isToolTabId(toolTabId("diff", "ws", "a.ts"))).toBe(true);
    expect(isToolTabId("agent-one")).toBe(false);
    expect(isToolTabId("session-2")).toBe(false);
    expect(isToolTabId("")).toBe(false);
  });

  it("mints the tab id from the workspace alone, so the host never reaches it", () => {
    expect(makeToolTab("diff", at("ws"), "a.ts").id).toBe("tool:diff:ws:a.ts");
    expect(makeToolTab("file", at("ws"), "a.ts").id).toBe(toolTabId("file", "ws", "a.ts"));
  });
});

describe("openToolTabs", () => {
  it("appends a new tab at the end", () => {
    const first = makeToolTab("diff", at("ws"), "a.ts");
    const tabs = openToolTabs([], first);
    expect(tabs.map((tab) => tab.id)).toEqual([first.id]);
    const second = makeToolTab("file", at("ws"), "b.ts");
    expect(openToolTabs(tabs, second).map((tab) => tab.id)).toEqual([first.id, second.id]);
  });

  it("opening the same kind, workspace and path again returns the list unchanged", () => {
    const tabs = [makeToolTab("diff", at("ws"), "a.ts")];
    expect(openToolTabs(tabs, makeToolTab("diff", at("ws"), "a.ts"))).toBe(tabs);
  });

  it("the same path in the other kind is a second tab", () => {
    const tabs = openToolTabs(
      [makeToolTab("diff", at("ws"), "a.ts")],
      makeToolTab("file", at("ws"), "a.ts"),
    );
    expect(tabs).toHaveLength(2);
  });
});

describe("toolTabLabel", () => {
  it("shows the basename of a path, and the id of a browser tab", () => {
    expect(toolTabLabel(makeToolTab("diff", at("ws"), "src/writer.ts"))).toBe("writer.ts");
    expect(toolTabLabel(makeToolTab("file", at("ws"), "writer.ts"))).toBe("writer.ts");
    expect(toolTabLabel(makeBrowserTab(at("ws"), "browser-1"))).toBe("browser-1");
  });
});

describe("makeBrowserTab", () => {
  it("gives one browser tab one id, in the tool namespace", () => {
    const tab = makeBrowserTab(at("ws"), "abc");
    expect(tab.kind).toBe("browser");
    expect(tab.id).toBe(toolTabId("browser", "ws", "abc"));
    expect(isToolTabId(tab.id)).toBe(true);
  });

  it("keeps two workspaces' browser tabs apart", () => {
    expect(makeBrowserTab(at("one"), "abc").id).not.toBe(makeBrowserTab(at("two"), "abc").id);
  });
});

describe("composeStripTabs", () => {
  it("holds sessions first, tool tabs appended", () => {
    const tabs = composeStripTabs(
      [session("s1"), session("s2")],
      [makeToolTab("diff", at("ws"), "a.ts")],
    );
    expect(tabs.map((tab) => tab.id)).toEqual(["s1", "s2", "tool:diff:ws:a.ts"]);
    expect(tabs[0].type).toBe("session");
    expect(tabs[2].type).toBe("tool");
  });
});

describe("successorOf", () => {
  const ids = ["s1", "s2", "s3"];
  it("takes the nearest survivor to the right", () => {
    expect(successorOf(ids, ["s2"], "s2")).toBe("s3");
  });
  it("falls to the left when nothing survives right", () => {
    expect(successorOf(ids, ["s3"], "s3")).toBe("s2");
    expect(successorOf(ids, ["s2", "s3"], "s3")).toBe("s1");
  });
  it("lands on nothing when every tab closed", () => {
    expect(successorOf(ids, ["s1", "s2", "s3"], "s2")).toBeNull();
  });
  it("leaves a tab the close did not take alone", () => {
    expect(successorOf(ids, ["s1"], "s2")).toBe("s2");
  });
  it("lands on the first survivor for an active id the strip does not hold", () => {
    expect(successorOf(ids, ["gone"], "gone")).toBe("s1");
    expect(successorOf(ids, ["s1", "gone"], "gone")).toBe("s2");
    expect(successorOf(ids, ["s1", "s2", "s3", "gone"], "gone")).toBeNull();
  });
});

describe("pruneToolTabsForWorkspaces", () => {
  const tabs: ToolTab[] = [
    makeToolTab("diff", at("ws-1"), "a.ts"),
    makeToolTab("file", at("ws-2"), "b.ts"),
  ];
  it("drops only the tabs of workspaces that are gone", () => {
    expect(pruneToolTabsForWorkspaces(tabs, new Set([at("ws-1"), at("ws-2")]))).toBe(tabs);
    expect(pruneToolTabsForWorkspaces(tabs, new Set([at("ws-1")])).map((tab) => tab.id)).toEqual([
      tabs[0].id,
    ]);
    expect(pruneToolTabsForWorkspaces(tabs, new Set())).toEqual([]);
  });

  it("prunes on the whole key, so another host's same workspace id cannot keep a tab", () => {
    const mine = [makeToolTab("diff", at("ws"), "a.ts")];
    expect(pruneToolTabsForWorkspaces(mine, new Set([mine[0].workspaceKey]))).toBe(mine);
    expect(pruneToolTabsForWorkspaces(mine, new Set([workspaceKey(PEER, "ws")!]))).toEqual([]);
  });
});
