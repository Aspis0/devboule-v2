// The tool-tab model: the id namespace, open/dedupe, labels, strip order,
// the shared successor rule and workspace pruning.

import { describe, expect, it } from "vitest";
import {
  composeStripTabs,
  isToolTabId,
  makeToolTab,
  openToolTabs,
  pruneToolTabsForWorkspaces,
  restoreToolTabs,
  successorOf,
  toolTabId,
  toolTabLabel,
  type ToolTab,
} from "./toolTabs";
import type { Session } from "../../../types/ipc";

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
});

describe("openToolTabs", () => {
  it("appends a new tab at the end", () => {
    const first = makeToolTab("diff", "ws", "a.ts");
    const tabs = openToolTabs([], first);
    expect(tabs.map((tab) => tab.id)).toEqual([first.id]);
    const second = makeToolTab("file", "ws", "b.ts");
    expect(openToolTabs(tabs, second).map((tab) => tab.id)).toEqual([first.id, second.id]);
  });

  it("opening the same kind, workspace and path again returns the list unchanged", () => {
    const tabs = [makeToolTab("diff", "ws", "a.ts")];
    expect(openToolTabs(tabs, makeToolTab("diff", "ws", "a.ts"))).toBe(tabs);
  });

  it("the same path in the other kind is a second tab", () => {
    const tabs = openToolTabs(
      [makeToolTab("diff", "ws", "a.ts")],
      makeToolTab("file", "ws", "a.ts"),
    );
    expect(tabs).toHaveLength(2);
  });
});

describe("toolTabLabel", () => {
  it("shows the basename", () => {
    expect(toolTabLabel("src/writer.ts")).toBe("writer.ts");
    expect(toolTabLabel("writer.ts")).toBe("writer.ts");
  });
});

describe("composeStripTabs", () => {
  it("holds sessions first, tool tabs appended", () => {
    const tabs = composeStripTabs(
      [session("s1"), session("s2")],
      [makeToolTab("diff", "ws", "a.ts")],
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
    makeToolTab("diff", "ws-1", "a.ts"),
    makeToolTab("file", "ws-2", "b.ts"),
  ];
  it("drops only the tabs of workspaces that are gone", () => {
    expect(pruneToolTabsForWorkspaces(tabs, new Set(["ws-1", "ws-2"]))).toBe(tabs);
    expect(pruneToolTabsForWorkspaces(tabs, new Set(["ws-1"])).map((tab) => tab.id)).toEqual([
      tabs[0].id,
    ]);
    expect(pruneToolTabsForWorkspaces(tabs, new Set())).toEqual([]);
  });
});

describe("restoreToolTabs", () => {
  const known = new Set(["ws"]);
  it("puts each tab back at the index it held", () => {
    const tabs = [
      makeToolTab("diff", "ws", "a.ts"),
      makeToolTab("diff", "ws", "b.ts"),
      makeToolTab("diff", "ws", "c.ts"),
    ];
    const prev = [tabs[1]];
    const removed = [
      { tab: tabs[0], index: 0 },
      { tab: tabs[2], index: 2 },
    ];
    expect(restoreToolTabs(prev, removed, known).map((tab) => tab.id)).toEqual(
      tabs.map((tab) => tab.id),
    );
  });

  it("leaves a reopened tab alone and appends tabs opened since at the end", () => {
    const first = makeToolTab("diff", "ws", "a.ts");
    const second = makeToolTab("diff", "ws", "b.ts");
    const fresh = makeToolTab("diff", "ws", "c.ts");
    const prev = [second, fresh];
    const next = restoreToolTabs(prev, [{ tab: first, index: 0 }], known);
    expect(next.map((tab) => tab.id)).toEqual([first.id, second.id, fresh.id]);
    expect(restoreToolTabs(next, [{ tab: second, index: 1 }], known)).toBe(next);
  });

  it("clamps a stale index to the end, behind tabs opened since", () => {
    const first = makeToolTab("diff", "ws", "a.ts");
    const fresh = makeToolTab("diff", "ws", "c.ts");
    const next = restoreToolTabs([fresh], [{ tab: first, index: 7 }], known);
    expect(next.map((tab) => tab.id)).toEqual([fresh.id, first.id]);
  });

  it("drops tabs whose workspace is no longer known", () => {
    const kept = makeToolTab("diff", "ws", "a.ts");
    const orphan = makeToolTab("diff", "gone", "b.ts");
    const next = restoreToolTabs(
      [],
      [
        { tab: kept, index: 0 },
        { tab: orphan, index: 1 },
      ],
      known,
    );
    expect(next.map((tab) => tab.id)).toEqual([kept.id]);
  });
});
