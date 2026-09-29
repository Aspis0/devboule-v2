// Why: the rename half of the tab menu — the dialog's open state, when the
// roster has moved under it, and the menu entry that opens it — lives here so
// the close flow owns the close and this owns the rename. The daemon's one
// rename road reaches the record only through a live process, so the entry is
// agent-only, capability-gated and refused on a journal-replayed (recovered)
// session; the state follows the roster between opens.

import { useCallback, useState } from "react";
import { isAgentKind, type Session } from "../../../types/ipc";
import { sessionTitle } from "../workspaceSessions";
import type { TabMenuEntry } from "./tabCloseMenu";

/** The open rename dialog: which session, and the name it currently shows. */
export interface SessionRenameTarget {
  sessionId: string;
  title: string;
}

interface UseSessionRenameArgs {
  sessions: readonly Session[];
  /** Whether the daemon advertises the capability that gates the rename
   * frame — the caller reads it off the daemon status. */
  renameSupported: boolean;
}

export function useSessionRename({ sessions, renameSupported }: UseSessionRenameArgs): {
  rename: SessionRenameTarget | null;
  openRename: (sessionId: string) => void;
  closeRename: () => void;
  renameEntriesFor: (anchorId: string) => TabMenuEntry[];
} {
  const [renameState, setRenameState] = useState<SessionRenameTarget | null>(null);

  // A MEANINGFUL roster change dismisses the rename the same way the close
  // flow dismisses the menu and the ask: the session is gone, so the dialog
  // closes with it rather than saving into a session that no longer exists.
  if (renameState !== null && !renameIsValid(renameState, sessions)) {
    setRenameState(null);
  }

  // The dialog's pre-fill follows the roster: a push that names the session
  // (the auto-title landing) moves the field with it. Adjusted during render,
  // not in an effect — the title is derived from the
  // roster this render already has.
  if (renameState !== null) {
    const row = sessions.find((session) => session.id === renameState.sessionId);
    if (row !== undefined) {
      const title = sessionTitle(row);
      if (title !== renameState.title) {
        setRenameState({ sessionId: renameState.sessionId, title });
      }
    }
  }

  // The rename dialog opens on the anchor's session; the title it pre-fills
  // is the name the surfaces already show.
  const openRename = useCallback(
    (sessionId: string) => {
      const row = sessions.find((session) => session.id === sessionId);
      if (row === undefined) return;
      setRenameState({ sessionId, title: sessionTitle(row) });
    },
    [sessions],
  );

  const closeRename = useCallback(() => setRenameState(null), []);

  const renameEntriesFor = useCallback(
    (anchorId: string) => renameEntryFor(anchorId, sessions, renameSupported),
    [sessions, renameSupported],
  );

  return { rename: renameState, openRename, closeRename, renameEntriesFor };
}

/** The menu's Rename entry for an agent tab when the daemon can rename —
 * an empty list otherwise. The daemon's one rename road reaches the record
 * only through a live process: a journal-replayed (recovered) session is a
 * Transcript entry and is refused with process_gone, while an agent that
 * merely ended stays a Live entry and renames fine. */
function renameEntryFor(
  anchorId: string,
  sessions: readonly Session[],
  renameSupported: boolean,
): TabMenuEntry[] {
  const row = sessions.find((session) => session.id === anchorId);
  if (!renameSupported || row === undefined || row.state.type === "recovered") return [];
  if (!isAgentKind(row.kind)) return [];
  const entry: TabMenuEntry = {
    key: "rename",
    label: "Rename",
    disabled: false,
    separatorAfter: true,
  };
  return [entry];
}

/** Whether the rename dialog's session is still on the roster — the same
 * question the close flow's menuIsValid and confirmIsValid ask of the
 * surfaces beside it. */
function renameIsValid(state: SessionRenameTarget, sessions: readonly Session[]): boolean {
  return sessions.some((session) => session.id === state.sessionId);
}
