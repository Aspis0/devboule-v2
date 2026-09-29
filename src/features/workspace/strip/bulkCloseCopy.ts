// Why: the confirmations' words in one place — Paseo's titles where they are
// ours, adapted where our semantics differ: for us a terminal close is an
// archive (session_stop), so the process stops and every message stays in
// History, where Paseo destroys the closed terminal.

import { isAgentKind, type Session } from "../../../types/ipc";
import { sessionHasHumanTitle, sessionKindWord, sessionTitle } from "../workspaceSessions";

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
  return `${bulkSelectionConfirmLabel(count)}?`;
}

/** The counted ask's confirm label: its title without the question mark —
 * the button matches what the title asks. */
export function bulkSelectionConfirmLabel(count: number): string {
  return `Close ${count} tab${count === 1 ? "" : "s"}`;
}

/** A mixed set's ask: the sessions keep today's archive copy, and the tool
 * tabs ride along in their own sentence — closing, never archived. With no
 * tool tabs the answer is byte-identical to `bulkCloseMessage`. */
export function mixedBulkCloseMessage(counts: BulkCloseCounts, toolCount: number): string {
  const base = bulkCloseMessage(counts);
  if (toolCount <= 0) return base;
  const tabs = toolCount === 1 ? "1 tab closes too." : `${toolCount} tabs close too.`;
  return `${base} ${tabs}`;
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

/** What an ask quotes: a shown title that only repeats the kind's default
 * word is the daemon's stamp, not a name. */
function quotableTitle(session: Session): string | null {
  if (!sessionHasHumanTitle(session)) return null;
  const shown = sessionTitle(session);
  if (shown.trim().toLowerCase() === sessionKindWord(session.kind).toLowerCase()) return null;
  return shown;
}

/** A single terminal close: the title names the shell the tab shows — the
 * tab's own title, so the ask and the chip never name one terminal two ways.
 * A shell that never set a title, or one still carrying the daemon's
 * kind-word stamp, has no name to quote: the ask reads
 * `Close terminal?`, never the machine's kind-and-id fallback. */
export function closeTerminalConfirm(session: Session): {
  title: string;
  message: string;
  confirmLabel: string;
} {
  const quoted = quotableTitle(session);
  const title = quoted !== null ? `Close terminal “${quoted}”?` : "Close terminal?";
  return {
    title,
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

/** A delete destroys the session: a named session is quoted, a nameless one
 * — or one still carrying the daemon's kind-word stamp — reads
 * `Delete terminal?` / `Delete agent?`, never the machine fallback. */
export function deleteSessionConfirm(session: Session): {
  title: string;
  message: string;
  confirmLabel: string;
} {
  const quoted = quotableTitle(session);
  const title =
    quoted !== null
      ? `Delete “${quoted}”?`
      : `Delete ${sessionKindWord(session.kind).toLowerCase()}?`;
  return {
    title,
    message: "This destroys the session and stops its running process immediately.",
    confirmLabel: "Delete",
  };
}
