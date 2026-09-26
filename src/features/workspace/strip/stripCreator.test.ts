import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { sessionCreatorTooltip } from "../workspaceSessions";

function terminal(id: string, title: string, createdBy?: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal",
    title,
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    ...(createdBy === undefined ? {} : { createdBy }),
  };
}

describe("sessionCreatorTooltip", () => {
  it("stays silent for a session a person started", () => {
    expect(sessionCreatorTooltip(terminal("t-1", "zsh"), [])).toBeNull();
  });

  it("names the creator by its visible title, never a raw id", () => {
    const creator: Session = {
      ...terminal("creator-1", "ignored fallback"),
      kind: "acp",
      displayName: "planner",
    };
    const child = terminal("t-1", "zsh", "creator-1");
    const tooltip = sessionCreatorTooltip(child, [creator, child]);
    expect(tooltip).toBe("created by planner");
    expect(tooltip).not.toContain("creator-1");
  });

  it("falls back to the creator's title when it has no display name", () => {
    const creator = terminal("creator-1", "worker one");
    const child = terminal("t-1", "zsh", "creator-1");
    expect(sessionCreatorTooltip(child, [creator, child])).toBe("created by worker one");
  });

  it("never leaks a raw id, even truncated, when the creator is gone", () => {
    const child = terminal("t-1", "zsh", "s.probably-gone");
    const tooltip = sessionCreatorTooltip(child, [child]);
    expect(tooltip).toBe("created by an agent");
    expect(tooltip).not.toContain("s.pro");
  });
});
