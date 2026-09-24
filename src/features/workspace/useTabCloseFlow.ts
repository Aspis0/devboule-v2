// Why: everything that runs when a close is set in motion — the tab menu's
// entries, when a close asks first (the close policy), what a confirmation
// resolves at the moment of confirming, and where focus and selection land
// afterwards — lives here so Workspace only wires it to the strip. Every
// close is an archive (session_stop) except a delete (session_close), and
// nothing waits for a window: the owner's decision took the undo away.

import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { isAgentKind, type Session } from "../../types/ipc";
import { sessionsForSelection, sessionsForTabAction } from "./bulkCloseSessions";
import type { CloseIntent } from "./closePolicy";
import { closeNeedsConfirmation } from "./closePolicy";
import {
  archiveRunningAgentConfirm,
  bulkActionTitle,
  bulkCloseMessage,
  bulkSelectionTitle,
  closeTerminalConfirm,
  countSessions,
  deleteSessionConfirm,
} from "./bulkCloseCopy";
import { sessionTitle } from "./workspaceSessions";
import { buildSelectionCloseEntry, buildTabCloseEntries, type TabMenuEntry } from "./tabCloseMenu";

/** The DOM id of a session's tab. Workspace renders it; the flow's focus
 * restore looks it up after the closed tabs have left the strip. */
export function sessionTabElementId(sessionId: string): string {
  return `workspace-session-tab-${sessionId}`;
}

interface TabCloseFlowArgs {
  sessions: readonly Session[];
  selectedSessionId: string | null;
  selection: ReadonlySet<string>;
  /** The acts, wired to the store by Workspace: matched targets fire, a
   * target that went stale between the ask and the click is reported, and
   * `onFailed` names a target whose act was refused (its row comes back). */
  onClose: (
    kind: CloseIntent,
    matched: readonly Session[],
    skipped: ReadonlyArray<{ id: string; title: string; generation: number }>,
    onFailed?: (sessionId: string) => void,
  ) => void;
  selectSession: (id: string | null) => void;
  clearSelection: () => void;
  addButtonRef: RefObject<HTMLButtonElement | null>;
}

/** A target named at ask time: resolved again, by id AND generation, at the
 * moment of confirming — only the confirmed instance acts. */
interface ConfirmTarget {
  readonly id: string;
  readonly generation: number;
}

interface CloseConfirmState {
  kind: CloseIntent;
  title: string;
  message: string;
  confirmLabel: string;
  targets: readonly ConfirmTarget[];
  /** The ask acts on the multi-selection: confirming it ends the selection. */
  actsOnSelection?: boolean;
}

// The menu records what it would act on — the selection for a selection
// menu, the anchor for a tab menu — with generations. It survives harmless
// republications and closes when ANY of its targets is removed or changes
// generation, because then its entries would act on a world the user has
// not seen.
interface MenuState {
  anchorId: string;
  anchorGeneration: number;
  viaSelection: boolean;
  targets: readonly ConfirmTarget[];
}

// Where focus goes when the flow closes something. "anchor" is the tab the
// act came from — when an ask is cancelled; "active" is the tab that is
// active after the close.
interface FocusRestore {
  kind: "anchor" | "active";
}

function targetOf(session: Session): ConfirmTarget {
  return { id: session.id, generation: session.state.generation };
}

function resolveConfirm(
  state: CloseConfirmState,
  sessions: readonly Session[],
): { matched: Session[]; skipped: Array<{ id: string; title: string; generation: number }> } {
  const matched: Session[] = [];
  const skipped: Array<{ id: string; title: string; generation: number }> = [];
  for (const target of state.targets) {
    const row = sessions.find((session) => session.id === target.id);
    if (row === undefined || row.state.generation !== target.generation) {
      skipped.push({
        id: target.id,
        title: row?.title ?? target.id,
        generation: target.generation,
      });
      continue;
    }
    matched.push(row);
  }
  return { matched, skipped };
}

/** The menu's live target set, in roster order — what it would act on NOW. */
function liveMenuTargets(
  state: MenuState,
  selection: ReadonlySet<string>,
  sessions: readonly Session[],
): ConfirmTarget[] {
  if (state.viaSelection) return sessionsForSelection(selection, sessions).map(targetOf);
  const anchor = sessions.find((session) => session.id === state.anchorId);
  return anchor === undefined ? [] : [targetOf(anchor)];
}

function sameTargets(a: readonly ConfirmTarget[], b: readonly ConfirmTarget[]): boolean {
  return a.length === b.length && a.every((target, index) => sameTarget(target, b[index] ?? null));
}

function sameTarget(a: ConfirmTarget, b: ConfirmTarget | null): boolean {
  return b !== null && a.id === b.id && a.generation === b.generation;
}

function menuIsValid(
  state: MenuState | null,
  selection: ReadonlySet<string>,
  sessions: readonly Session[],
): boolean {
  if (state === null) return false;
  return sameTargets(state.targets, liveMenuTargets(state, selection, sessions));
}

function confirmIsValid(state: CloseConfirmState | null, sessions: readonly Session[]): boolean {
  if (state === null) return false;
  return state.targets.every((target) => {
    const row = sessions.find((session) => session.id === target.id);
    return row !== undefined && row.state.generation === target.generation;
  });
}

export function useTabCloseFlow({
  sessions,
  selectedSessionId,
  selection,
  onClose,
  selectSession,
  clearSelection,
  addButtonRef,
}: TabCloseFlowArgs): {
  menu: { sessionId: string; entries: TabMenuEntry[] } | null;
  anchorRef: RefObject<HTMLElement | null>;
  confirm: CloseConfirmState | null;
  openMenu: (sessionId: string) => void;
  closeMenu: () => void;
  closeSingle: (sessionId: string) => void;
  activateEntry: (key: TabMenuEntry["key"]) => void;
  confirmClose: () => void;
  cancelClose: () => void;
} {
  const [menuState, setMenuState] = useState<MenuState | null>(null);
  const [confirmState, setConfirmState] = useState<CloseConfirmState | null>(null);
  const [focusRestore, setFocusRestore] = useState<FocusRestore | null>(null);
  const anchorRef = useRef<HTMLElement | null>(null);

  const openMenuState = menuIsValid(menuState, selection, sessions) ? menuState : null;
  const menu =
    openMenuState === null
      ? null
      : {
          sessionId: openMenuState.anchorId,
          entries: openMenuState.viaSelection
            ? [buildSelectionCloseEntry(openMenuState.targets.length)]
            : buildTabCloseEntries(
                sessions.findIndex((session) => session.id === openMenuState.anchorId),
                sessions.length,
              ),
        };
  const confirm = confirmIsValid(confirmState, sessions) ? confirmState : null;

  // A menu or an ask dismissed by a MEANINGFUL roster change is dead, not
  // dormant: the invalid state is cleared in the same render that hides it
  // (React's adjust-state-when-a-prop-changes form), or a removed target's
  // later return with the same generation could mount the old surface again
  // with no new action from the user.
  const [dismissedMenu, setDismissedMenu] = useState<MenuState | null>(null);
  const [dismissedConfirm, setDismissedConfirm] = useState<CloseConfirmState | null>(null);
  if (menuState !== null && !menuIsValid(menuState, selection, sessions)) {
    setDismissedMenu(menuState);
    setMenuState(null);
  }
  if (confirmState !== null && !confirmIsValid(confirmState, sessions)) {
    setDismissedConfirm(confirmState);
    setConfirmState(null);
  }

  // Paseo's rule for the tab that takes over when a close took the active
  // one (getCloseSuccessorTabId, applied to the closed set): the nearest
  // survivor to the RIGHT of the closed active tab, else the nearest to the
  // left; with none left, no active tab — the empty state. Every close path
  // goes through this, not just the bulk ones.
  const finishClose = useCallback(
    (closedIds: readonly string[]) => {
      if (selectedSessionId === null || !closedIds.includes(selectedSessionId)) {
        return;
      }
      const activeIndex = sessions.findIndex((session) => session.id === selectedSessionId);
      const closed = new Set(closedIds);
      const survivor =
        sessions.slice(activeIndex + 1).find((session) => !closed.has(session.id)) ??
        [...sessions.slice(0, activeIndex)].reverse().find((session) => !closed.has(session.id));
      selectSession(survivor?.id ?? null);
    },
    [selectSession, selectedSessionId, sessions],
  );

  // Selection and focus move to the successor as the close fires; when the
  // act is REFUSED, its row comes back — and if it was the active one, the
  // selection and focus come back with it, or the strip would show an empty
  // state beside a restored tab.
  const restoreOnFailure = useCallback(
    (activeAtClose: string | null) => (failedId: string) => {
      if (failedId !== activeAtClose) return;
      selectSession(failedId);
      setFocusRestore({ kind: "active" });
    },
    [selectSession],
  );

  const openConfirm = useCallback((state: CloseConfirmState) => setConfirmState(state), []);

  const closeSingle = useCallback(
    (sessionId: string) => {
      anchorRef.current = document.getElementById(sessionTabElementId(sessionId));
      const row = sessions.find((session) => session.id === sessionId);
      if (row === undefined) return;
      if (!closeNeedsConfirmation(row, "archive")) {
        // No process behind the row (ended, recovered): nothing is running,
        // so the archive fires at once — no ask, and no window to undo in.
        onClose("archive", [row], [], restoreOnFailure(row.id));
        finishClose([row.id]);
        setFocusRestore({ kind: "active" });
        return;
      }
      if (isAgentKind(row.kind)) {
        openConfirm({
          kind: "archive",
          targets: [targetOf(row)],
          ...archiveRunningAgentConfirm(),
        });
        return;
      }
      openConfirm({ kind: "archive", targets: [targetOf(row)], ...closeTerminalConfirm() });
    },
    [finishClose, onClose, openConfirm, restoreOnFailure, sessions],
  );

  const openMenu = useCallback(
    (sessionId: string) => {
      // The anchor is the TAB BUTTON, the focusable thing: Escape and a
      // cancelled ask hand focus back to it, and the popovers place from it.
      anchorRef.current = document.getElementById(sessionTabElementId(sessionId));
      const row = sessions.find((session) => session.id === sessionId);
      if (row === undefined) return;
      const viaSelection = selection.has(sessionId);
      setMenuState({
        anchorId: sessionId,
        anchorGeneration: row.state.generation,
        viaSelection,
        targets: viaSelection
          ? sessionsForSelection(selection, sessions).map(targetOf)
          : [targetOf(row)],
      });
    },
    [selection, sessions],
  );

  const closeMenu = useCallback(() => setMenuState(null), []);

  const activateEntry = useCallback(
    (key: TabMenuEntry["key"]) => {
      // Only the menu's own buttons get here, so menuState stands for the
      // open menu; reading the state (not the derived object) keeps this
      // callback stable across renders.
      const open = menuState;
      setMenuState(null);
      if (open === null) return;
      const anchorId = open.anchorId;
      if (key === "close") {
        closeSingle(anchorId);
        return;
      }
      if (key === "delete") {
        // Delete destroys the session, so it always asks, whatever is
        // running. Never offered on a selection menu.
        const row = sessions.find((session) => session.id === anchorId);
        if (row === undefined) return;
        openConfirm({
          kind: "delete",
          targets: [targetOf(row)],
          ...deleteSessionConfirm(sessionTitle(row)),
        });
        return;
      }
      if (key === "close-selection") {
        // The selection menu ALWAYS asks, even when the roster has shrunk it
        // to one: the ask lists the live set, so the user confirms what is
        // really there, not what the count said when the selection was made.
        // The selection itself ends only when the ask is CONFIRMED — a
        // cancel leaves it exactly as it was.
        const closed = sessionsForSelection(selection, sessions);
        if (closed.length === 0) return;
        openConfirm({
          kind: "archive",
          targets: closed.map(targetOf),
          title: bulkSelectionTitle(closed.length),
          message: bulkCloseMessage(countSessions(closed)),
          confirmLabel: "Close",
          actsOnSelection: true,
        });
        return;
      }
      const closed = sessionsForTabAction(key, sessions, anchorId);
      if (closed.length === 0) return;
      openConfirm({
        kind: "archive",
        targets: closed.map(targetOf),
        title: bulkActionTitle(key),
        message: bulkCloseMessage(countSessions(closed)),
        confirmLabel: "Close",
      });
    },
    [closeSingle, menuState, openConfirm, selection, sessions],
  );

  const confirmClose = useCallback(() => {
    const current = confirm;
    setConfirmState(null);
    if (current === null) return;
    // Resolve at the moment of confirming, by id AND generation: only the
    // instances the user confirmed act; a vanished or resumed target is
    // reported, never touched.
    const { matched, skipped } = resolveConfirm(current, sessions);
    if (matched.length > 0) {
      onClose(current.kind, matched, skipped, restoreOnFailure(selectedSessionId));
    }
    if (current.actsOnSelection === true) clearSelection();
    finishClose(matched.map((session) => session.id));
    setFocusRestore({ kind: "active" });
  }, [
    clearSelection,
    confirm,
    finishClose,
    onClose,
    restoreOnFailure,
    selectedSessionId,
    sessions,
  ]);

  const cancelClose = useCallback(() => {
    setConfirmState(null);
    // Cancelled: nothing was fired, so the tab the ask came from is still
    // there to take focus back.
    setFocusRestore({ kind: "anchor" });
  }, []);

  // The restore runs in an effect on purpose: it must read the DOM after the
  // commit that hid the closed rows, so the chain below asks the elements —
  // not the state — what survived. A closed anchor falls through to the
  // active tab, and with no active tab focus lands on "+". Each intent is a
  // fresh object, applied once; the last one stays in state, where it is
  // never rendered.
  const appliedRestore = useRef<FocusRestore | null>(null);
  const restoreFocus = useCallback(
    (kind: FocusRestore["kind"]) => {
      const anchor = kind === "anchor" ? anchorRef.current : null;
      const target =
        anchor !== null && anchor.isConnected
          ? anchor
          : selectedSessionId !== null
            ? document.getElementById(sessionTabElementId(selectedSessionId))
            : null;
      (target ?? addButtonRef.current)?.focus({ preventScroll: true });
    },
    [addButtonRef, selectedSessionId],
  );
  useEffect(() => {
    if (focusRestore === null || appliedRestore.current === focusRestore) return;
    appliedRestore.current = focusRestore;
    restoreFocus(focusRestore.kind);
  }, [focusRestore, restoreFocus]);

  // A MEANINGFUL roster change dismissed the menu or the ask. The surface is
  // already invisible and cleared, so only focus is left to mend, and it is
  // mended by the same close path a cancel takes. Each dismissal marker is
  // a fresh object, handled once.
  const handledDismissal = useRef<MenuState | CloseConfirmState | null>(null);
  useEffect(() => {
    const dismissed = dismissedMenu ?? dismissedConfirm;
    if (dismissed === null || handledDismissal.current === dismissed) return;
    handledDismissal.current = dismissed;
    restoreFocus("anchor");
  }, [dismissedConfirm, dismissedMenu, restoreFocus]);

  return {
    menu,
    anchorRef,
    confirm,
    openMenu,
    closeMenu,
    closeSingle,
    activateEntry,
    confirmClose,
    cancelClose,
  };
}
