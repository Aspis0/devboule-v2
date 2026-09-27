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

function byId(...sessions: Session[]): Map<string, Session> {
  return new Map(sessions.map((session) => [session.id, session]));
}

describe("sessionCreatorTooltip", () => {
  it("stays silent for a session a person started", () => {
    expect(sessionCreatorTooltip(terminal("t-1", "zsh"), byId())).toBeNull();
  });

  it("names the creator by its visible title, never a raw id", () => {
    const creator: Session = {
      ...terminal("creator-1", "ignored fallback"),
      kind: "acp",
      displayName: "planner",
    };
    const child = terminal("t-1", "zsh", "creator-1");
    const tooltip = sessionCreatorTooltip(child, byId(creator, child));
    expect(tooltip).toBe("created by planner");
    expect(tooltip).not.toContain("creator-1");
  });

  it("falls back to the creator's title when it has no display name", () => {
    const creator = terminal("creator-1", "worker one");
    const child = terminal("t-1", "zsh", "creator-1");
    expect(sessionCreatorTooltip(child, byId(creator, child))).toBe("created by worker one");
  });

  it("never leaks a raw id, even truncated, when the creator is gone", () => {
    const child = terminal("t-1", "zsh", "s.probably-gone");
    const tooltip = sessionCreatorTooltip(child, byId(child));
    expect(tooltip).toBe("created by an agent");
    expect(tooltip).not.toContain("s.pro");
  });

  it("never leaks a raw id when the creator lives on another device's roster", () => {
    const child = terminal("t-1", "zsh", "peer-device-session-9");
    const tooltip = sessionCreatorTooltip(child, byId(child));
    expect(tooltip).toBe("created by an agent");
    expect(tooltip).not.toContain("peer-device-session-9");
  });

  it("never leaks a raw id when the creator row has no title", () => {
    const creator = terminal("creator-1", "");
    const child = terminal("t-1", "zsh", "creator-1");
    const tooltip = sessionCreatorTooltip(child, byId(creator, child));
    expect(tooltip).toBe("created by an agent");
    expect(tooltip).not.toContain("creator-1");
  });

  it("never leaks a raw id when the creator row's title is blank", () => {
    const creator = terminal("creator-1", "   ");
    const child = terminal("t-1", "zsh", "creator-1");
    const tooltip = sessionCreatorTooltip(child, byId(creator, child));
    expect(tooltip).toBe("created by an agent");
    expect(tooltip).not.toContain("creator-1");
  });
});
