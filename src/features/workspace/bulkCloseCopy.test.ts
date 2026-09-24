// The bulk close's confirmation: Paseo's titles verbatim, and the counting
// line adapted only where our semantics differ — for us a terminal close is
// an archive too (session_stop), so the process stops and every message
// stays in History, where Paseo destroys the closed terminal.

import { describe, expect, it } from "vitest";
import type { Session } from "../../types/ipc";
import {
  archiveRunningAgentConfirm,
  bulkActionTitle,
  bulkCloseMessage,
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

describe("bulkActionTitle", () => {
  it("is Paseo's title for each tab-relative action", () => {
    expect(bulkActionTitle("left")).toBe("Close tabs to the left?");
    expect(bulkActionTitle("right")).toBe("Close tabs to the right?");
    expect(bulkActionTitle("others")).toBe("Close other tabs?");
  });
});

describe("bulkSelectionTitle", () => {
  it("counts the selection — our own title, Paseo has no multi-select", () => {
    expect(bulkSelectionTitle(4)).toBe("Close 4 tabs?");
  });

  it("asks about one tab in the singular: the ask names the live set", () => {
    expect(bulkSelectionTitle(1)).toBe("Close 1 tab?");
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

  it("agents only: Paseo's line, unchanged", () => {
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
  it("a terminal: Paseo's title verbatim, our archive's sentence", () => {
    expect(closeTerminalConfirm()).toEqual({
      title: "Close terminal?",
      message: "The process stops and every message stays in History.",
      confirmLabel: "Close",
    });
  });

  it("a running agent: Paseo's title verbatim, our sentence keeps the transcript", () => {
    expect(archiveRunningAgentConfirm()).toEqual({
      title: "Archive running agent?",
      message:
        "This agent is still running. Archiving it stops the agent; every message stays in History.",
      confirmLabel: "Archive",
    });
  });

  it("a delete names the session and says what destruction means", () => {
    expect(deleteSessionConfirm("shell two")).toEqual({
      title: "Delete “shell two”?",
      message: "This destroys the session and stops its running process immediately.",
      confirmLabel: "Delete",
    });
  });
});
