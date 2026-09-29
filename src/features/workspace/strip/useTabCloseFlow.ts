// Why: everything that runs when a close is set in motion — the tab menu's
// entries, when a close asks first (the close policy), what a confirmation
// resolves at the moment of confirming, and where focus and selection land
// afterwards — lives here so Workspace only wires it to the strip. Every
// session close is an archive (session_stop) except a delete
// (session_close), and nothing waits for a window: the owner's decision took
// the undo away. A tool tab's close is a local removal instead — no confirm,
// no daemon call — but it runs the same successor rule against the composed
// strip, so focus and selection land exactly as they do for a session tab.

import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { isAgentKind, type Session } from "../../../types/ipc";
import { sessionsForSelection, sessionsForTabAction } from "./bulkCloseSessions";
import type { CloseIntent } from "./closePolicy";
import { closeNeedsConfirmation } from "./closePolicy";
import {
  archiveRunningAgentConfirm,
  bulkActionTitle,
  bulkSelectionConfirmLabel,
  bulkSelectionTitle,
  closeTerminalConfirm,
  countSessions,
  deleteSessionConfirm,
  mixedBulkCloseMessage,
} from "./bulkCloseCopy";
import { buildSelectionCloseEntry, buildTabCloseEntries, type TabMenuEntry } from "./tabCloseMenu";
import { successorOf, type StripTab, type ToolTab } from "./toolTabs";
import { planBulkClose } from "./tabClosePlan";
import { toolTabMenuEntries } from "./toolTabMenu";

/** The DOM id of a strip tab. Workspace renders it; the flow's focus
 * restore looks it up after the closed tabs have left the strip. Sessions
 * and tool tabs share the scheme, so the keyboard and the restore need no
 * kind check to find either. */
export function sessionTabElementId(tabId: string): string {
  return `workspace-session-tab-${tabId}`;
}

interface TabCloseFlowArgs {
  sessions: readonly Session[];
  /** The composed strip Workspace owns — sessions first, tool tabs appended.
   * The one list every close slices and lands on; the tool half derives below. */
  tabs: readonly StripTab[];
  /** The ONE active tab id both readers — chips, keyboard, successor — use. */
  activeTabId: string | null;
  selection: ReadonlySet<string>;
  /** The acts, wired to the store by Workspace: matched targets fire now,
   * and `onFailed` names a target whose act was refused (its row came back). */
  onClose: (
    kind: CloseIntent,
    matched: readonly Session[],
    onFailed?: (sessionId: string) => void,
  ) => void;
  /** A tool tab's removal, wired to the tab list by Workspace: local only,
   * never a daemon call. Returns the restore for a mixed close to call
   * when its session part fails. */
  onCloseTools: (ids: readonly string[]) => () => void;
  selectTab: (id: string | null) => void;
  clearSelection: () => void;
  addButtonRef: RefObject<HTMLButtonElement | null>;
  /** The rename half of the tab menu, wired by the caller from the rename
   * hook: the entries for an anchor and the open for the menu's rename key.
   * Required: a caller that forgets it must not compile into a menu that
   * silently lost its rename row. */
  renameMenu: RenameMenu;
}

/** The rename half of the tab menu as the caller wires it from the rename
 * hook. */
interface RenameMenu {
  entriesFor: (anchorId: string) => TabMenuEntry[];
  open: (anchorId: string) => void;
}

/** A target named at ask time: resolved again, by id AND generation, at the
 * moment of confirming — only the confirmed instance acts. Tool tabs carry
 * no generation; their id is the identity, so it stands in as one. */
interface ConfirmTarget {
  readonly id: string;
  readonly generation: number;
}

interface CloseConfirmState {
  kind: CloseIntent;
  title: string;
  message: string;
  confirmLabel: string;
  /** The affirmative's fill. Every ask the flow raises is a destructive act
   * (close, archive, delete), so the sole affirmative is the filled danger. */
  tone: "danger" | "accent";
  targets: readonly ConfirmTarget[];
  /** Tool tabs the confirmed close takes with it: they never ask, never
   * touch the daemon, and they close with the confirm, not before it — a
   * cancel is a true no-op for both kinds. */
  toolTargets: readonly string[];
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

function targetOf(tab: StripTab): ConfirmTarget {
  return {
    id: tab.id,
    generation: tab.type === "session" ? tab.session.state.generation : 0,
  };
}

function resolveConfirm(
  state: CloseConfirmState,
  sessions: readonly Session[],
  tools: readonly ToolTab[],
): {
  matched: Session[];
  matchedTools: string[];
} {
  const matched: Session[] = [];
  for (const target of state.targets) {
    const row = sessions.find((session) => session.id === target.id);
    if (row === undefined || row.state.generation !== target.generation) continue;
    matched.push(row);
  }
  const present = new Set(tools.map((tool) => tool.id));
  const matchedTools = state.toolTargets.filter((id) => present.has(id));
  return { matched, matchedTools };
}

/** The menu's live target set, in strip order — what it would act on NOW. */
function liveMenuTargets(
  state: MenuState,
  selection: ReadonlySet<string>,
  tabs: readonly StripTab[],
): ConfirmTarget[] {
  if (state.viaSelection) return sessionsForSelection(selection, tabs).map(targetOf);
  const anchor = tabs.find((tab) => tab.id === state.anchorId);
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
  tabs: readonly StripTab[],
): boolean {
  if (state === null) return false;
  return sameTargets(state.targets, liveMenuTargets(state, selection, tabs));
}

function confirmIsValid(
  state: CloseConfirmState | null,
  sessions: readonly Session[],
  tools: readonly ToolTab[],
): boolean {
  if (state === null) return false;
  const presentTools = new Set(tools.map((tool) => tool.id));
  return (
    state.targets.every((target) => {
      const row = sessions.find((session) => session.id === target.id);
      return row !== undefined && row.state.generation === target.generation;
    }) && state.toolTargets.every((id) => presentTools.has(id))
  );
}

export function useTabCloseFlow({
  sessions,
  tabs,
  activeTabId,
  selection,
  onClose,
  onCloseTools,
  selectTab,
  clearSelection,
  addButtonRef,
  renameMenu,
}: TabCloseFlowArgs): {
  menu: { anchorId: string; entries: TabMenuEntry[] } | null;
  anchorRef: RefObject<HTMLElement | null>;
  confirm: CloseConfirmState | null;
  openMenu: (tabId: string) => void;
  closeMenu: () => void;
  closeSingle: (tabId: string) => void;
  activateEntry: (key: TabMenuEntry["key"]) => void;
  activatePaneEntry: (anchorId: string, key: TabMenuEntry["key"]) => void;
  confirmClose: () => void;
  cancelClose: () => void;
} {
  const [menuState, setMenuState] = useState<MenuState | null>(null);
  const [confirmState, setConfirmState] = useState<CloseConfirmState | null>(null);
  const [focusRestore, setFocusRestore] = useState<FocusRestore | null>(null);
  const anchorRef = useRef<HTMLElement | null>(null);

  // The rename half, destructured so the callbacks below keep stable deps:
  // the wrapper the caller passes is a fresh object per render, the functions
  // inside it are not.
  const renameEntriesFor = renameMenu?.entriesFor;
  const openRename = renameMenu?.open;

  // The tool half of the composed strip above. A pure session strip derives
  // an empty one, so every path below reads identically with no tool tabs.
  const tools = useMemo(
    () => tabs.flatMap((tab) => (tab.type === "tool" ? [tab.tool] : [])),
    [tabs],
  );

  const openMenuState = menuIsValid(menuState, selection, tabs) ? menuState : null;
  const anchorTab =
    openMenuState === null ? undefined : tabs.find((tab) => tab.id === openMenuState.anchorId);
  // Rename sits ahead of the close group on an agent tab;
  // a selection menu and a tool tab's menu are close-only. The capability
  // gate lives in the rename hook's entry builder, not here.
  const menu =
    openMenuState === null || anchorTab === undefined
      ? null
      : {
          anchorId: openMenuState.anchorId,
          entries: openMenuState.viaSelection
            ? [buildSelectionCloseEntry(openMenuState.targets.length)]
            : (toolTabMenuEntries(tabs, openMenuState.anchorId) ?? [
                ...(renameEntriesFor?.(openMenuState.anchorId) ?? []),
                ...buildTabCloseEntries(
                  tabs.findIndex((tab) => tab.id === openMenuState.anchorId),
                  tabs.length,
                ),
              ]),
        };
  const confirm = confirmIsValid(confirmState, sessions, tools) ? confirmState : null;

  // A menu or an ask dismissed by a MEANINGFUL roster change is dead, not
  // dormant: the invalid state is cleared in the same render that hides it
  // (React's adjust-state-when-a-prop-changes form), or a removed target's
  // later return with the same generation could mount the old surface again
  // with no new action from the user.
  const [dismissedMenu, setDismissedMenu] = useState<MenuState | null>(null);
  const [dismissedConfirm, setDismissedConfirm] = useState<CloseConfirmState | null>(null);
  if (menuState !== null && !menuIsValid(menuState, selection, tabs)) {
    setDismissedMenu(menuState);
    setMenuState(null);
  }
  if (confirmState !== null && !confirmIsValid(confirmState, sessions, tools)) {
    setDismissedConfirm(confirmState);
    setConfirmState(null);
  }

  const finishClose = useCallback(
    (closedIds: readonly string[]) => {
      const next = successorOf(
        tabs.map((tab) => tab.id),
        closedIds,
        activeTabId ?? "",
      );
      if (activeTabId === null || next === activeTabId) return;
      selectTab(next);
    },
    [selectTab, activeTabId, tabs],
  );

  // Selection and focus move to the successor as the close fires; when the
  // act is REFUSED, its row comes back — and if it was the active one, the
  // selection and focus come back with it, or the strip would show an empty
  // state beside a restored tab.
  const restoreOnFailure = useCallback(
    (activeAtClose: string | null) => (failedId: string) => {
      if (failedId !== activeAtClose) return;
      selectTab(failedId);
      setFocusRestore({ kind: "active" });
    },
    [selectTab],
  );

  const openConfirm = useCallback((state: CloseConfirmState) => setConfirmState(state), []);

  const requestFocus = useCallback(() => setFocusRestore({ kind: "active" }), []);
  // A tools-only close is local and cannot fail: remove, land, refocus.
  const closeTools = useCallback(
    (ids: readonly string[]) => {
      if (ids.length === 0) return;
      onCloseTools(ids);
      finishClose(ids);
      requestFocus();
    },
    [finishClose, onCloseTools, requestFocus],
  );

  const closeSingle = useCallback(
    (tabId: string) => {
      anchorRef.current = document.getElementById(sessionTabElementId(tabId));
      const tool = tools.find((candidate) => candidate.id === tabId);
      if (tool !== undefined) {
        // Nothing to lose: a tool tab's close is a local removal — no ask,
        // no daemon call — and still lands on the shared successor.
        closeTools([tool.id]);
        return;
      }
      const row = sessions.find((session) => session.id === tabId);
      if (row === undefined) return;
      if (!closeNeedsConfirmation(row, "archive")) {
        // No process behind the row (ended, recovered): nothing is running,
        // so the archive fires at once — no ask, and no window to undo in.
        onClose("archive", [row], restoreOnFailure(row.id));
        finishClose([row.id]);
        setFocusRestore({ kind: "active" });
        return;
      }
      if (isAgentKind(row.kind)) {
        openConfirm({
          kind: "archive",
          targets: [targetOf({ type: "session", id: row.id, session: row })],
          toolTargets: [],
          ...archiveRunningAgentConfirm(),
          tone: "danger",
        });
        return;
      }
      openConfirm({
        kind: "archive",
        targets: [targetOf({ type: "session", id: row.id, session: row })],
        toolTargets: [],
        ...closeTerminalConfirm(row),
        tone: "danger",
      });
    },
    [closeTools, finishClose, onClose, openConfirm, restoreOnFailure, sessions, tools],
  );

  const openMenu = useCallback(
    (tabId: string) => {
      // The anchor is the TAB BUTTON, the focusable thing: Escape and a
      // cancelled ask hand focus back to it, and the popovers place from it.
      anchorRef.current = document.getElementById(sessionTabElementId(tabId));
      const tab = tabs.find((candidate) => candidate.id === tabId);
      if (tab === undefined) return;
      const viaSelection = selection.has(tabId);
      setMenuState({
        anchorId: tabId,
        anchorGeneration: tab.type === "session" ? tab.session.state.generation : 0,
        viaSelection,
        targets: viaSelection
          ? sessionsForSelection(selection, tabs).map(targetOf)
          : [targetOf(tab)],
      });
    },
    [selection, tabs],
  );

  const closeMenu = useCallback(() => setMenuState(null), []);

  // The per-anchor core both entry points share: the tab menu resolves its
  // anchor from its own open state, the pane header names its session.
  const fireAnchorEntry = useCallback(
    (anchorId: string, key: TabMenuEntry["key"]) => {
      if (key === "close") {
        closeSingle(anchorId);
        return;
      }
      if (key === "delete") {
        // Delete destroys the session, so it always asks, whatever is
        // running. Never offered on a selection menu — and never on a tool
        // tab, where there is no session to destroy.
        const row = sessions.find((session) => session.id === anchorId);
        if (row === undefined) return;
        openConfirm({
          kind: "delete",
          targets: [targetOf({ type: "session", id: row.id, session: row })],
          toolTargets: [],
          ...deleteSessionConfirm(row),
          tone: "danger",
        });
        return;
      }
      if (key === "close-selection") return;
      // Rename is not a close: both entry points open the dialog before this
      // core runs, so a rename key here is a caller that skipped its own
      // guard — refused, never fed to the close policy.
      if (key === "rename") return;
      const plan = planBulkClose(sessionsForTabAction(key, tabs, anchorId));
      if (plan.kind === "nothing") return;
      if (plan.kind === "tools-only") {
        // Nothing to ask about, so they close at once, locally.
        closeTools(plan.toolVictims);
        return;
      }
      openConfirm({
        kind: "archive",
        targets: plan.sessionVictims.map((session) =>
          targetOf({ type: "session", id: session.id, session }),
        ),
        toolTargets: plan.toolVictims,
        tone: "danger",
        title: bulkActionTitle(key),
        message: mixedBulkCloseMessage(countSessions(plan.sessionVictims), plan.toolVictims.length),
        confirmLabel: "Close",
      });
    },
    [closeSingle, closeTools, openConfirm, tabs, sessions],
  );

  const activateEntry = useCallback(
    (key: TabMenuEntry["key"]) => {
      // Only the menu's own buttons get here, so menuState stands for the
      // open menu; reading the state (not the derived object) keeps this
      // callback stable across renders.
      const open = menuState;
      setMenuState(null);
      if (open === null) return;
      if (key === "rename") {
        // Focus the tab before the dialog takes it: the dialog returns focus
        // to whatever held it when it opened.
        anchorRef.current?.focus({ preventScroll: true });
        openRename?.(open.anchorId);
        return;
      }
      if (key === "close-selection") {
        // The selection menu ALWAYS asks, even when the roster has shrunk it
        // to one: the ask lists the live set, so the user confirms what is
        // really there, not what the count said when the selection was made.
        // The selection itself ends only when the ask is CONFIRMED — a
        // cancel leaves it exactly as it was. A tools-only selection never
        // asks: there is no session policy to keep.
        const plan = planBulkClose(sessionsForSelection(selection, tabs));
        if (plan.kind === "nothing") return;
        if (plan.kind === "tools-only") {
          closeTools(plan.toolVictims);
          return;
        }
        const total = plan.sessionVictims.length + plan.toolVictims.length;
        openConfirm({
          kind: "archive",
          targets: plan.sessionVictims.map((session) =>
            targetOf({ type: "session", id: session.id, session }),
          ),
          toolTargets: plan.toolVictims,
          tone: "danger",
          title: bulkSelectionTitle(total),
          message: mixedBulkCloseMessage(
            countSessions(plan.sessionVictims),
            plan.toolVictims.length,
          ),
          confirmLabel: bulkSelectionConfirmLabel(total),
          actsOnSelection: true,
        });
        return;
      }
      fireAnchorEntry(open.anchorId, key);
    },
    [fireAnchorEntry, menuState, openConfirm, openRename, selection, tabs, closeTools],
  );

  // The pane header's kebab fires through the same policy and confirmation
  // as the tab menu, anchored at its own session instead of the tab menu's.
  // Keys the header never offers (selection, delete) are refused here.
  const activatePaneEntry = useCallback(
    (anchorId: string, key: TabMenuEntry["key"]) => {
      if (key === "close-selection" || key === "delete") return;
      if (key === "rename") {
        openRename?.(anchorId);
        return;
      }
      fireAnchorEntry(anchorId, key);
    },
    [fireAnchorEntry, openRename],
  );

  const confirmClose = useCallback(() => {
    const current = confirm;
    setConfirmState(null);
    if (current === null) return;
    // Resolve at the moment of confirming, by id AND generation: only the
    // instances the user confirmed act.
    const { matched, matchedTools } = resolveConfirm(current, sessions, tools);
    const sessionIds = matched.map((session) => session.id);
    if (current.actsOnSelection === true) clearSelection();
    // The tools go at once, with the successor and the focus, in the same
    // tick as the click — nothing waits for the daemon. A refused session
    // act brings its row back and puts the tools back at their indices,
    // without moving selection or focus.
    let restoreMixed: (() => void) | null = null;
    const onFailed = (failedId: string) => {
      restoreMixed?.();
      restoreMixed = null;
      restoreOnFailure(activeTabId)(failedId);
    };
    onClose(current.kind, matched, onFailed);
    restoreMixed = onCloseTools(matchedTools);
    finishClose([...sessionIds, ...matchedTools]);
    setFocusRestore({ kind: "active" });
  }, [
    clearSelection,
    confirm,
    finishClose,
    onClose,
    onCloseTools,
    restoreOnFailure,
    activeTabId,
    sessions,
    tools,
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
          : activeTabId !== null
            ? document.getElementById(sessionTabElementId(activeTabId))
            : null;
      (target ?? addButtonRef.current)?.focus({ preventScroll: true });
    },
    [addButtonRef, activeTabId],
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
    activatePaneEntry,
    confirmClose,
    cancelClose,
  };
}
