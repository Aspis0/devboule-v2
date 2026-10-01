import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { sessionTitle } from "../workspaceSessions";
import {
  archiveRunningAgentConfirm,
  closeTerminalConfirm,
  deleteSessionConfirm,
} from "./bulkCloseCopy";

function terminalSession(id: string, title: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "terminal",
    title,
    state: { type: "live", generation: 1 } as Session["state"],
    elapsedMs: 0,
  };
}

function agentSession(id: string, title: string): Session {
  return {
    id,
    workspaceId: "workspace-1",
    kind: "codex",
    title,
    state: { type: "live", generation: 1 } as Session["state"],
    elapsedMs: 0,
  };
}

describe("the single-close confirmations", () => {
  it("a terminal: names the shell the tab shows, our archive's sentence", () => {
    expect(closeTerminalConfirm(terminalSession("session-2", "shell two"))).toEqual({
      title: `Close terminal “shell two”?`,
      message: "The process stops and every message stays in History.",
      confirmLabel: "Close",
    });
  });

  it("a terminal whose shell never set a title: the ask reads Close terminal?", () => {
    expect(closeTerminalConfirm(terminalSession("4f2a1b3c", ""))).toEqual({
      title: "Close terminal?",
      message: "The process stops and every message stays in History.",
      confirmLabel: "Close",
    });
  });

  it("a title that only repeats the kind word is the daemon's stamp, not a name", () => {
    expect(closeTerminalConfirm(terminalSession("session-9", "Terminal"))).toEqual({
      title: "Close terminal?",
      message: "The process stops and every message stays in History.",
      confirmLabel: "Close",
    });
  });

  it("the kind-word match is trimmed and case-insensitive", () => {
    expect(closeTerminalConfirm(terminalSession("session-9", " terminal "))).toEqual({
      title: "Close terminal?",
      message: "The process stops and every message stays in History.",
      confirmLabel: "Close",
    });
  });

  it("a real shell name still quotes", () => {
    expect(closeTerminalConfirm(terminalSession("session-9", "zsh"))).toEqual({
      title: `Close terminal “zsh”?`,
      message: "The process stops and every message stays in History.",
      confirmLabel: "Close",
    });
  });

  it("the chip keeps showing the stamped word as the tab label", () => {
    expect(sessionTitle(terminalSession("session-9", "Terminal"))).toBe("Terminal");
  });

  it("a running agent: asks to archive, keeps the transcript in History", () => {
    expect(archiveRunningAgentConfirm()).toEqual({
      title: "Archive running agent?",
      message:
        "This agent is still running. Archiving it stops the agent; every message stays in History.",
      confirmLabel: "Archive",
    });
  });

  it("a delete with a human title quotes it", () => {
    expect(deleteSessionConfirm(terminalSession("session-2", "shell two"))).toEqual({
      title: "Delete “shell two”?",
      message: "This destroys the session and stops its running process immediately.",
      confirmLabel: "Delete",
    });
  });

  it("a delete without a title names the terminal kind, never the fallback", () => {
    expect(deleteSessionConfirm(terminalSession("4f2a1b3c", ""))).toEqual({
      title: "Delete terminal?",
      message: "This destroys the session and stops its running process immediately.",
      confirmLabel: "Delete",
    });
  });

  it("a delete without a title names the agent kind, never the fallback", () => {
    expect(deleteSessionConfirm(agentSession("9d1c2e4f", ""))).toEqual({
      title: "Delete agent?",
      message: "This destroys the session and stops its running process immediately.",
      confirmLabel: "Delete",
    });
  });

  it("a delete of a terminal still carrying the stamp names the kind", () => {
    expect(deleteSessionConfirm(terminalSession("4f2a1b3c", "Terminal"))).toEqual({
      title: "Delete terminal?",
      message: "This destroys the session and stops its running process immediately.",
      confirmLabel: "Delete",
    });
  });

  it("a delete of an agent still carrying the stamp names the kind", () => {
    expect(deleteSessionConfirm(agentSession("9d1c2e4f", "Agent"))).toEqual({
      title: "Delete agent?",
      message: "This destroys the session and stops its running process immediately.",
      confirmLabel: "Delete",
    });
  });

  it("a delete quotes the other kind's word: it is not this session's stamp", () => {
    expect(deleteSessionConfirm(agentSession("9d1c2e4f", "Terminal"))).toEqual({
      title: "Delete “Terminal”?",
      message: "This destroys the session and stops its running process immediately.",
      confirmLabel: "Delete",
    });
  });
});
