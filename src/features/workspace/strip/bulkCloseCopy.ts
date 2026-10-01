import type { Session } from "../../../types/ipc";
import { sessionHasHumanTitle, sessionKindWord, sessionTitle } from "../workspaceSessions";

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

/** A single running agent: a fixed title; the message says the archive
 * stops the agent and every message stays in History. */
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
