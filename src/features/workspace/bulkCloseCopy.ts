// Why: the confirmations' words in one place — Paseo's titles verbatim, its
// counting line adapted only where our semantics differ: for us a terminal
// close is an archive (session_stop), so the process stops and every message
// stays in History, where Paseo destroys the closed terminal.

import { isAgentKind, type Session } from "../../types/ipc";

interface BulkCloseCounts {
  agents: number;
  terminals: number;
}

export function countSessions(sessions: readonly Session[]): BulkCloseCounts {
  const counts: BulkCloseCounts = { agents: 0, terminals: 0 };
  for (const session of sessions) {
    if (isAgentKind(session.kind)) counts.agents += 1;
    else counts.terminals += 1;
  }
  return counts;
}

/** Paseo's titles, verbatim (workspace.tabs.confirmations.closeTabs*Title). */
export function bulkActionTitle(action: "left" | "right" | "others"): string {
  if (action === "left") return "Close tabs to the left?";
  if (action === "right") return "Close tabs to the right?";
  return "Close other tabs?";
}

/** Our own title: Paseo has no multi-select. The live set is what is asked
 * about, even when a roster change shrank the selection to one. */
export function bulkSelectionTitle(count: number): string {
  return `Close ${count} tab${count === 1 ? "" : "s"}?`;
}

export function bulkCloseMessage(counts: BulkCloseCounts): string {
  const { agents, terminals } = counts;
  if (agents > 0 && terminals > 0) {
    return `This will archive ${agents} agent(s) and archive ${terminals} terminal(s). The processes stop and every message stays in History.`;
  }
  if (terminals > 0) {
    return `This will archive ${terminals} terminal(s). The processes stop and every message stays in History.`;
  }
  return `This will archive ${agents} agent(s).`;
}

/** A single terminal close: Paseo's title verbatim; the message adapted to
 * what our close is — an archive, so the messages stay. */
export function closeTerminalConfirm(): { title: string; message: string; confirmLabel: string } {
  return {
    title: "Close terminal?",
    message: "The process stops and every message stays in History.",
    confirmLabel: "Close",
  };
}

/** A single running agent: Paseo's title verbatim; the message says our
 * close keeps the transcript, where Paseo's also closes the tab for good. */
export function archiveRunningAgentConfirm(): {
  title: string;
  message: string;
  confirmLabel: string;
} {
  return {
    title: "Archive running agent?",
    message:
      "This agent is still running. Archiving it stops the agent; every message stays in History.",
    confirmLabel: "Archive",
  };
}

/** A delete destroys the session — the tab pill's own words, reused. */
export function deleteSessionConfirm(title: string): {
  title: string;
  message: string;
  confirmLabel: string;
} {
  return {
    title: `Delete “${title}”?`,
    message: "This destroys the session and stops its running process immediately.",
    confirmLabel: "Delete",
  };
}
