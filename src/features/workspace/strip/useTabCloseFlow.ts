// Tab closes remove membership locally; pane Close and Delete apply session lifecycle policy.
import { useMenuBranch } from "../useMenuBranch";
import { localWorkspaceKey } from "../hosts/hostIdentity";
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";
import { isAgentKind, type Session } from "../../../types/ipc";
import { sessionsForSelection, sessionsForTabAction } from "./bulkCloseSessions";
import type { CloseIntent } from "./closePolicy";
import { closeNeedsConfirmation } from "./closePolicy";
import {
  archiveRunningAgentConfirm,
  closeTerminalConfirm,
  deleteSessionConfirm,
} from "./bulkCloseCopy";
import { buildSelectionCloseEntry, buildTabCloseEntries, type TabMenuEntry } from "./tabCloseMenu";
import { successorOf, type StripTab } from "./toolTabs";
import { toolTabMenuEntries } from "./toolTabMenu";
import {
  buildTabCopyEntries,
  isTabCopyAction,
  tabCopyValue,
  type TabCopyAction,
} from "./tabCopyActions";

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
   * The one list every close slices and lands on; sessions and tools share it. */
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
  onCloseTools: (ids: readonly string[]) => void;
  onCloseTabs: (ids: readonly string[]) => void;
  selectTab: (id: string | null) => void;
  clearSelection: () => void;
  addButtonRef: RefObject<HTMLButtonElement | null>;
  /** Where a browser tab's page is, so its menu row offers an address
   * instead of the file path a file tab has. */
  resolveBrowserAddress?: (browserId: string) => string | null;
  /** The rename half of the tab menu, wired by the caller from the rename
   * hook: the entries for an anchor and the open for the menu's rename key.
   * Required: a caller that forgets it must not compile into a menu that
   * silently lost its rename row. */
  renameMenu: RenameMenu;
  /** The tab in the pane below, so the menu can name the act that brings it
   * back up. */
  lowerTabId?: string | null;
  /** The two pane acts the menu offers, wired to the layout that answers them.
   * Optional: a caller with no split wires neither and the menu offers neither. */
  onSplitDown?: (tabId: string) => void;
  onMoveUpPane?: (tabId: string) => void;
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

function resolveConfirm(state: CloseConfirmState, sessions: readonly Session[]): Session[] {
  const matched: Session[] = [];
  for (const target of state.targets) {
    const row = sessions.find((session) => session.id === target.id);
    if (row === undefined || row.state.generation !== target.generation) continue;
    matched.push(row);
  }
  return matched;
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

function confirmIsValid(state: CloseConfirmState | null, sessions: readonly Session[]): boolean {
  if (state === null) return false;
  return state.targets.every((target) => {
    const row = sessions.find((session) => session.id === target.id);
    return row !== undefined && row.state.generation === target.generation;
  });
}

export function useTabCloseFlow({
  sessions,
  tabs,
  activeTabId,
  selection,
  onClose,
  onCloseTools,
  onCloseTabs,
  selectTab,
  clearSelection,
  addButtonRef,
  resolveBrowserAddress,
  renameMenu,
  lowerTabId = null,
  onSplitDown,
  onMoveUpPane,
}: TabCloseFlowArgs): {
  menu: { anchorId: string; entries: TabMenuEntry[] } | null;
  anchorRef: RefObject<HTMLElement | null>;
  confirm: CloseConfirmState | null;
  openMenu: (tabId: string) => void;
  closeMenu: () => void;
  closeSingle: (tabId: string) => void;
  closeTab: (tabId: string) => void;
  activateEntry: (key: TabMenuEntry["key"]) => void;
  copyEntryValue: (key: TabCopyAction) => string | null;
  activatePaneEntry: (anchorId: string, key: TabMenuEntry["key"]) => void;
  confirmClose: () => void;
  cancelClose: () => void;
} {
  const [menuState, setMenuState] = useState<MenuState | null>(null);
  const [confirmState, setConfirmState] = useState<CloseConfirmState | null>(null);
  const [focusRestore, setFocusRestore] = useState<FocusRestore | null>(null);
  const anchorRef = useRef<HTMLElement | null>(null);
  const activeRef = useRef(activeTabId);
  useLayoutEffect(() => {
    activeRef.current = activeTabId;
  }, [activeTabId]);

  // The rename half, destructured so the callbacks below keep stable deps:
  // the wrapper the caller passes is a fresh object per render, the functions
  // inside it are not.
  const renameEntriesFor = renameMenu.entriesFor;
  const openRename = renameMenu.open;

  const openMenuState = menuIsValid(menuState, selection, tabs) ? menuState : null;
  const anchorTab =
    openMenuState === null ? undefined : tabs.find((tab) => tab.id === openMenuState.anchorId);
  const anchorWorkspaceId = anchorTab?.type === "session" ? anchorTab.session.workspaceId : null;
  const branch = useMenuBranch(
    anchorWorkspaceId === null || openMenuState?.viaSelection
      ? null
      : localWorkspaceKey(anchorWorkspaceId),
    openMenuState !== null && !openMenuState.viaSelection,
  );
  // Selection menus act on the whole selection; copies name a single anchor.
  const menu =
    openMenuState === null || anchorTab === undefined
      ? null
      : {
          anchorId: openMenuState.anchorId,
          entries: openMenuState.viaSelection
            ? [buildSelectionCloseEntry(openMenuState.targets.length)]
            : (toolTabMenuEntries(tabs, openMenuState.anchorId, resolveBrowserAddress, {
                isBelow: openMenuState.anchorId === lowerTabId,
              }) ?? [
                ...buildTabCopyEntries(anchorTab, branch, resolveBrowserAddress),
                ...renameEntriesFor(openMenuState.anchorId),
                ...buildTabCloseEntries(
                  tabs.findIndex((tab) => tab.id === openMenuState.anchorId),
                  tabs.length,
                ),
              ]),
        };
  const confirm = confirmIsValid(confirmState, sessions) ? confirmState : null;

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
  if (confirmState !== null && !confirmIsValid(confirmState, sessions)) {
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
      activeRef.current = next;
      selectTab(next);
    },
    [selectTab, activeTabId, tabs],
  );

  // A refusal restores the active tab only while the user still occupies its successor.
  const restoreOnFailure = useCallback(
    (activeAtClose: string | null) => {
      const successor = successorOf(
        tabs.map((tab) => tab.id),
        [activeAtClose ?? ""],
        activeAtClose ?? "",
      );
      return (failedId: string) => {
        if (failedId !== activeAtClose || activeRef.current !== successor) return;
        selectTab(failedId);
        setFocusRestore({ kind: "active" });
      };
    },
    [selectTab, tabs],
  );

  const openConfirm = useCallback((state: CloseConfirmState) => setConfirmState(state), []);

  const requestFocus = useCallback(() => setFocusRestore({ kind: "active" }), []);
  const closeSingle = useCallback(
    (tabId: string) => {
      anchorRef.current = document.getElementById(sessionTabElementId(tabId));
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
          ...archiveRunningAgentConfirm(),
          tone: "danger",
        });
        return;
      }
      openConfirm({
        kind: "archive",
        targets: [targetOf({ type: "session", id: row.id, session: row })],
        ...closeTerminalConfirm(row),
        tone: "danger",
      });
    },
    [finishClose, onClose, openConfirm, restoreOnFailure, sessions],
  );

  const removeTabs = useCallback(
    (victims: readonly StripTab[]) => {
      onCloseTabs(victims.filter((tab) => tab.type === "session").map((tab) => tab.id));
      onCloseTools(victims.filter((tab) => tab.type === "tool").map((tab) => tab.id));
      finishClose(victims.map((tab) => tab.id));
      clearSelection();
      requestFocus();
    },
    [onCloseTabs, onCloseTools, finishClose, clearSelection, requestFocus],
  );

  const closeTab = useCallback(
    (tabId: string) => {
      removeTabs(tabs.filter((tab) => tab.id === tabId));
    },
    [removeTabs, tabs],
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

  const copyEntryValue = (key: TabCopyAction): string | null => {
    if (openMenuState === null || openMenuState.viaSelection || anchorTab === undefined)
      return null;
    return tabCopyValue(anchorTab, key, branch, resolveBrowserAddress);
  };

  // The per-anchor core both entry points share: the tab menu resolves its
  // anchor from its own open state, the pane header names its session.
  const fireAnchorEntry = useCallback(
    (anchorId: string, key: TabMenuEntry["key"]) => {
      if (key === "close" || key === "left" || key === "right" || key === "others") {
        removeTabs(
          key === "close"
            ? tabs.filter((tab) => tab.id === anchorId)
            : sessionsForTabAction(key, tabs, anchorId),
        );
        return;
      }
      if (key === "split-down") {
        // The keyboard's road into the split: the same act the drag's bottom
        // edge makes, on the tab the menu was opened from.
        onSplitDown?.(anchorId);
        return;
      }
      if (key === "move-up-pane") {
        onMoveUpPane?.(anchorId);
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
          ...deleteSessionConfirm(row),
          tone: "danger",
        });
        return;
      }
    },
    [onMoveUpPane, onSplitDown, openConfirm, tabs, sessions, removeTabs],
  );

  const activateEntry = useCallback(
    (key: TabMenuEntry["key"]) => {
      if (isTabCopyAction(key)) return;
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
        openRename(open.anchorId);
        return;
      }
      if (key === "close-selection") {
        removeTabs(sessionsForSelection(selection, tabs));
        return;
      }
      fireAnchorEntry(open.anchorId, key);
    },
    [fireAnchorEntry, menuState, openRename, selection, tabs, removeTabs],
  );

  // Pane Close stops the session; its other close entries remove tabs locally.
  const activatePaneEntry = useCallback(
    (anchorId: string, key: TabMenuEntry["key"]) => {
      if (isTabCopyAction(key) || key === "close-selection" || key === "delete") return;
      if (key === "rename") {
        openRename?.(anchorId);
        return;
      }
      if (key === "close") {
        closeSingle(anchorId);
        return;
      }
      fireAnchorEntry(anchorId, key);
    },
    [fireAnchorEntry, openRename, closeSingle],
  );

  const confirmClose = useCallback(() => {
    const current = confirm;
    setConfirmState(null);
    if (current === null) return;
    // Resolve at the moment of confirming, by id AND generation: only the
    // instances the user confirmed act.
    const matched = resolveConfirm(current, sessions);
    onClose(current.kind, matched, restoreOnFailure(activeTabId));
    finishClose(matched.map((session) => session.id));
    setFocusRestore({ kind: "active" });
  }, [confirm, finishClose, onClose, restoreOnFailure, activeTabId, sessions]);

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
    closeTab,
    activateEntry,
    copyEntryValue,
    activatePaneEntry,
    confirmClose,
    cancelClose,
  };
}
