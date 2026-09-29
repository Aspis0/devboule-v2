// The bulk close's confirmation: Paseo's titles verbatim, and the counting
// line adapted only where our semantics differ — for us a terminal close is
// an archive too (session_stop), so the process stops and every message
// stays in History, where Paseo destroys the closed terminal.

import { describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { sessionTitle } from "../workspaceSessions";
import {
  archiveRunningAgentConfirm,
  bulkActionTitle,
  bulkCloseMessage,
  bulkSelectionConfirmLabel,
  bulkSelectionTitle,
  closeTerminalConfirm,
  countSessions,
  deleteSessionConfirm,
} from "./bulkCloseCopy";

function sessions(kinds: Session["kind"][]): Session[] {
  return kinds.map((kind, index) => ({
    id: `s${index}`,
    workspaceId: "workspace-1",
    kind,
    title: `${kind} ${index}`,
    state: { type: "live", generation: 1 } as Session["state"],
    elapsedMs: 0,
  }));
}

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

describe("bulkActionTitle", () => {
  it("titles each tab-relative action as a question", () => {
    expect(bulkActionTitle("left")).toBe("Close tabs to the left?");
    expect(bulkActionTitle("right")).toBe("Close tabs to the right?");
    expect(bulkActionTitle("others")).toBe("Close other tabs?");
  });
});

describe("bulkSelectionTitle", () => {
  it("counts the selection in our own multi-select title", () => {
    expect(bulkSelectionTitle(4)).toBe("Close 4 tabs?");
  });

  it("asks about one tab in the singular: the ask names the live set", () => {
    expect(bulkSelectionTitle(1)).toBe("Close 1 tab?");
  });
});

describe("bulkSelectionConfirmLabel", () => {
  it("is the title without the question mark: the button matches what the title asks", () => {
    expect(bulkSelectionConfirmLabel(3)).toBe("Close 3 tabs");
    expect(bulkSelectionTitle(3)).toBe(`${bulkSelectionConfirmLabel(3)}?`);
  });

  it("keeps the singular in step with the title", () => {
    expect(bulkSelectionConfirmLabel(1)).toBe("Close 1 tab");
  });
});

describe("countSessions", () => {
  it("splits agents from terminals by kind", () => {
    expect(countSessions(sessions(["acp", "claude", "terminal", "pi"]))).toEqual({
      agents: 3,
      terminals: 1,
    });
    expect(countSessions(sessions(["terminal"]))).toEqual({ agents: 0, terminals: 1 });
    expect(countSessions(sessions(["codex"]))).toEqual({ agents: 1, terminals: 0 });
  });
});

describe("bulkCloseMessage", () => {
  it("mixed: archives both kinds, and says the processes stop and the messages stay", () => {
    expect(bulkCloseMessage({ agents: 2, terminals: 1 })).toBe(
      "This will archive 2 agent(s) and archive 1 terminal(s). " +
        "The processes stop and every message stays in History.",
    );
  });

  it("agents only: names the archived agents", () => {
    expect(bulkCloseMessage({ agents: 3, terminals: 0 })).toBe("This will archive 3 agent(s).");
  });

  it("terminals only: archived, not closed, and History keeps the messages", () => {
    expect(bulkCloseMessage({ agents: 0, terminals: 2 })).toBe(
      "This will archive 2 terminal(s). " +
        "The processes stop and every message stays in History.",
    );
  });
});

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
