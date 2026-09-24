// Which sessions a close action takes: the strip's visible order, the
// anchor tab exclusive for left/right/others (Paseo's slicing), and the
// selection intersected with what is on screen.

import { describe, expect, it } from "vitest";
import type { Session } from "../../types/ipc";
import { sessionsForSelection, sessionsForTabAction } from "./bulkCloseSessions";

function session(id: string, kind: Session["kind"] = "terminal"): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind,
    title: id,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
  };
}

const strip = [session("s1", "acp"), session("s2"), session("s3"), session("s4")];

const ids = (sessions: Session[]): string[] => sessions.map((item) => item.id);

describe("sessionsForTabAction", () => {
  it("takes the tabs left of the anchor, exclusive", () => {
    expect(ids(sessionsForTabAction("left", strip, "s3"))).toEqual(["s1", "s2"]);
  });

  it("takes the tabs right of the anchor, exclusive", () => {
    expect(ids(sessionsForTabAction("right", strip, "s2"))).toEqual(["s3", "s4"]);
  });

  it("takes every tab but the anchor for others", () => {
    expect(ids(sessionsForTabAction("others", strip, "s2"))).toEqual(["s1", "s3", "s4"]);
  });

  it("takes only the anchor for close", () => {
    expect(ids(sessionsForTabAction("close", strip, "s2"))).toEqual(["s2"]);
    expect(ids(sessionsForTabAction("close", strip, "s1"))).toEqual(["s1"]);
  });
});

describe("sessionsForSelection", () => {
  it("keeps the strip's order, not the click order", () => {
    const selection = new Set(["s4", "s1", "s3"]);
    expect(ids(sessionsForSelection(selection, strip))).toEqual(["s1", "s3", "s4"]);
  });

  it("ignores ids no longer on screen", () => {
    const selection = new Set(["s2", "gone"]);
    expect(ids(sessionsForSelection(selection, strip))).toEqual(["s2"]);
  });
});
