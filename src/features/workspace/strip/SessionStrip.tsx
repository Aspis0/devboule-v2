import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import type { Session } from "../../../types/ipc";
import { ConfirmDialog } from "../../../components/ConfirmDialog";
import { sessionDelegationTakeBack, sessionOriginBadge } from "../workspaceSessions";
import type { useTabCloseFlow } from "./useTabCloseFlow";
import type { useTabSelection } from "./useTabSelection";
import { useSelectedTabVisible } from "./stripScroll";
import { SessionTabMenu } from "./SessionTabMenu";
import { SessionOverviewMenu } from "./SessionOverviewMenu";
import { WorkspaceNewTabMenu } from "./WorkspaceNewTabMenu";
import { chipDisplay } from "./stripDisplay";
import { sessionNeedsApproval } from "../sessionAttention";
import { useStripFade } from "./useStripFade";
import { useStripKeyboard } from "./useStripKeyboard";
import { StripChip, ToolStripChip, type BrowserTabPage } from "./StripChip";
import type { StripTab } from "./toolTabs";
import { browserLayoutSnapshot, subscribeBrowserLayout } from "../browserTabs";
import { browserPagesSnapshot, subscribeBrowserPages } from "../browserPages";
import { browserTabLabel } from "../browserUrl";
import "./strip.css";

/** Chips re-render only when their own props change: every other prop the
 * strip passes is stable across unrelated renders (see the callbacks
 * below), so a composer keystroke never walks the strip. */
const MemoStripChip = memo(StripChip);
const MemoToolChip = memo(ToolStripChip);

export interface StripNewTab {
  open: boolean;
  creating: boolean;
  workspaceSelected: boolean;
  onToggle: () => void;
  onAgent: () => void;
  onTerminal: () => void;
  onBrowser: () => void;
  onCloseMenu: () => void;
}

export interface SessionStripProps {
  /** The composed strip Workspace owns — sessions first, tool tabs appended.
   * The same array the selection and the close flow read: never recomposed. */
  tabs: StripTab[];
  /** The ONE active tab id both readers — chips, keyboard, scroll — use. */
  activeTabId: string | null;
  /** Routes a tab id to its owner: the session controller, or the tool list. */
  selectTab: (id: string) => void;
  tabSelection: ReturnType<typeof useTabSelection>;
  tabClose: ReturnType<typeof useTabCloseFlow>;
  addButtonRef: RefObject<HTMLButtonElement | null>;
  newTab: StripNewTab;
  /** The provider choice anchored at the strip, or null when it lives elsewhere. */
  providerMenu: ReactNode;
  peerNames: ReadonlyMap<string, string>;
  /** The tooltip's creator line for one session; Workspace binds the roster. */
  resolveCreator: (session: Session) => string | null;
  takeBackAvailable: boolean;
  onTakeBack: () => void;
  statusText: string;
  /** Every roster session of the selected workspace: the strip shows a
   * subset, the end-of-strip overview lists all of it. */
  overviewSessions: readonly Session[];
  /** The selected workspace's title for the overview preview. */
  workspaceName: string | null;
  /** Opens an overview row through Workspace's existing open path, by id. */
  onOpenSession: (sessionId: string) => void;
  /** The selected session id for the overview's initial row; the tab id
   * stays the strip's and may name a tool tab. */
  selectedSessionId: string | null;
}

/** The tab strip region: the scrolling tablist, the fade on the sides that
 * still hide chips, the "+" pinned after the scrollport, and the count. */
export function SessionStrip({
  tabs,
  activeTabId,
  selectTab,
  tabSelection,
  tabClose,
  addButtonRef,
  newTab,
  providerMenu,
  peerNames,
  resolveCreator,
  takeBackAvailable,
  onTakeBack,
  statusText,
  overviewSessions,
  workspaceName,
  onOpenSession,
  selectedSessionId,
}: SessionStripProps) {
  const scrollportRef = useRef<HTMLDivElement>(null);
  const sessions = useMemo(
    () => tabs.flatMap((tab) => (tab.type === "session" ? [tab.session] : [])),
    [tabs],
  );
  const openIds = useMemo(() => new Set(sessions.map((session) => session.id)), [sessions]);
  const unopenedAttentionCount = useMemo(
    () =>
      overviewSessions.filter(
        (session) => sessionNeedsApproval(session) && !openIds.has(session.id),
      ).length,
    [overviewSessions, openIds],
  );
  const attentionSummary =
    unopenedAttentionCount === 0
      ? ""
      : ` — ${unopenedAttentionCount} session${unopenedAttentionCount === 1 ? " needs" : "s need"} your approval`;
  const toolTabs = useMemo(
    () => tabs.flatMap((tab) => (tab.type === "tool" ? [tab.tool] : [])),
    [tabs],
  );
  // A browser chip names its page, and the favicon arrives long after the tab
  // does. The strip reads the tab model itself rather than taking a map
  // through Workspace, and `MemoToolChip` still keeps the other chips still.
  const browserRecords = useSyncExternalStore(subscribeBrowserLayout, browserLayoutSnapshot);
  const pageStates = useSyncExternalStore(subscribeBrowserPages, browserPagesSnapshot);
  const browserPages = useMemo(() => {
    return new Map<string, BrowserTabPage>(
      browserRecords.tabs.map((tab) => [
        tab.browserId,
        {
          label: browserTabLabel(tab.title, tab.url),
          favicon: tab.favicon,
          url: tab.url,
          // The live page, not the record: a tab restored after a restart has
          // a record and no page, so it is not loading.
          loading: pageStates.get(tab.browserId)?.loading ?? false,
        },
      ]),
    );
  }, [browserRecords.tabs, pageStates]);
  useSelectedTabVisible(scrollportRef, activeTabId, tabs);
  const fade = useStripFade(scrollportRef, tabs);
  const { selection, handleTabClick } = tabSelection;
  const { menu, openMenu, closeTab } = tabClose;
  const menuAnchorId = menu?.anchorId;
  const keyboard = useStripKeyboard({
    tabs,
    activeTabId,
    selectTab,
    closeTab,
  });
  const { tabIndexFor, onChipKeyDown: keyboardChipKeyDown } = keyboard;
  // The end-of-strip overview: the count is its trigger. Hover opens on
  // intent and survives the crossing into the list; press opens at once.
  // A click on a hover-opened list pins it instead of dismissing it.
  const [overviewOpen, setOverviewOpen] = useState(false);
  // Fresh mount per opening, so the menu's clock and seed start at open.
  const [overviewEpoch, setOverviewEpoch] = useState(0);
  const overviewTriggerRef = useRef<HTMLButtonElement>(null);
  const overviewRootRef = useRef<HTMLDivElement>(null);
  const overviewSourceRef = useRef<"hover" | "press" | null>(null);
  const overviewOpenTimer = useRef<number | null>(null);
  const overviewCloseTimer = useRef<number | null>(null);
  useEffect(
    () => () => {
      if (overviewOpenTimer.current !== null) window.clearTimeout(overviewOpenTimer.current);
      if (overviewCloseTimer.current !== null) window.clearTimeout(overviewCloseTimer.current);
    },
    [],
  );
  const cancelOverviewTimers = useCallback(() => {
    if (overviewOpenTimer.current !== null) {
      window.clearTimeout(overviewOpenTimer.current);
      overviewOpenTimer.current = null;
    }
    if (overviewCloseTimer.current !== null) {
      window.clearTimeout(overviewCloseTimer.current);
      overviewCloseTimer.current = null;
    }
  }, []);
  const { open: newTabOpen, onCloseMenu: closeNewTabMenu } = newTab;
  const { menu: tabMenu, closeMenu: closeTabMenu } = tabClose;
  const openOverview = useCallback(
    (source: "hover" | "press") => {
      cancelOverviewTimers();
      // The standing strip siblings yield to the overview; anything else
      // registered — a provider choice, a consent gate — stays standing.
      if (newTabOpen) closeNewTabMenu();
      if (tabMenu !== null) closeTabMenu();
      overviewSourceRef.current = source;
      setOverviewEpoch((epoch) => epoch + 1);
      setOverviewOpen(true);
    },
    [cancelOverviewTimers, newTabOpen, closeNewTabMenu, tabMenu, closeTabMenu],
  );
  // The one close every path runs through: focus comes back only when the
  // closing menu held it, so a hover-close never steals the pane's focus.
  const closeOverview = useCallback(() => {
    cancelOverviewTimers();
    overviewSourceRef.current = null;
    if (overviewRootRef.current?.contains(document.activeElement) === true) {
      overviewTriggerRef.current?.focus({ preventScroll: true });
    }
    setOverviewOpen(false);
  }, [cancelOverviewTimers]);
  const scheduleOverviewOpen = useCallback(() => {
    if (overviewCloseTimer.current !== null) {
      window.clearTimeout(overviewCloseTimer.current);
      overviewCloseTimer.current = null;
    }
    // A re-entry during the grace continues the standing menu: arming an
    // open would dismiss it against its own registration on fire.
    if (overviewOpen) return;
    if (overviewOpenTimer.current !== null) return;
    overviewOpenTimer.current = window.setTimeout(() => {
      overviewOpenTimer.current = null;
      openOverview("hover");
    }, 150);
  }, [openOverview, overviewOpen]);
  const scheduleOverviewClose = useCallback(() => {
    if (overviewOpenTimer.current !== null) {
      window.clearTimeout(overviewOpenTimer.current);
      overviewOpenTimer.current = null;
    }
    if (overviewCloseTimer.current !== null) return;
    overviewCloseTimer.current = window.setTimeout(closeOverview, 150);
  }, [closeOverview]);
  const stripOrder = useMemo(
    () => tabs.flatMap((tab) => (tab.type === "session" ? [tab.session.id] : [])),
    [tabs],
  );
  // The list's own count: open tabs first, then the sessions with no tab,
  // named as sessions so the count never mislabels either side.
  const overviewUnopenedCount = useMemo(
    () => overviewSessions.filter((session) => !openIds.has(session.id)).length,
    [overviewSessions, openIds],
  );
  const overviewTabsWord = tabs.length === 1 ? "tab" : "tabs";
  const overviewSessionWord = overviewUnopenedCount === 1 ? "session" : "sessions";
  const handleOverviewOpen = useCallback(
    (sessionId: string) => {
      closeOverview();
      onOpenSession(sessionId);
    },
    [closeOverview, onOpenSession],
  );
  const handleOverviewSelectTool = useCallback(
    (id: string) => {
      closeOverview();
      selectTab(id);
    },
    [closeOverview, selectTab],
  );

  const handleChipKeyDown = useCallback(
    (id: string, event: ReactKeyboardEvent<HTMLElement>) => {
      if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
        event.preventDefault();
        openMenu(id);
        return;
      }
      keyboardChipKeyDown(id, event);
    },
    [openMenu, keyboardChipKeyDown],
  );

  // One derivation per row, recomputed only when the roster, the names, or
  // the take-back switch change — a keystroke or daemon tick that leaves
  // them alone reuses the same dot, words and tooltip objects.
  const chips = useMemo(
    () =>
      sessions.map((session) => {
        const display = chipDisplay(session);
        const origin = sessionOriginBadge(session, peerNames) ?? undefined;
        const creator = resolveCreator(session) ?? undefined;
        const tooltip = [display.tooltip, origin, creator]
          .filter((line) => line !== undefined)
          .join("\n");
        return {
          session,
          display,
          tooltip,
          provenanceLines: [...display.detailLines, origin, creator].filter(
            (line) => line !== undefined,
          ),
          takeBack: takeBackAvailable && sessionDelegationTakeBack(session),
        };
      }),
    [sessions, peerNames, resolveCreator, takeBackAvailable],
  );

  // The rows themselves, memoised on the same stable inputs: an
  // unrelated parent render reuses the very same elements, so the
  // memoised chips below never re-render for it.
  const rows = useMemo(
    () => [
      ...chips.map(({ session, display, tooltip, provenanceLines, takeBack }) => (
        <MemoStripChip
          key={session.id}
          session={session}
          selected={activeTabId === session.id}
          multiselected={selection.has(session.id)}
          tabIndex={tabIndexFor(session.id)}
          display={display}
          tooltip={tooltip}
          provenanceLines={provenanceLines}
          menuOpen={menuAnchorId === session.id}
          takeBack={takeBack}
          onTakeBack={onTakeBack}
          onTabClick={(event) => handleTabClick(session, event)}
          onTabAuxClick={(event) => {
            if (event.button === 1) {
              event.preventDefault();
              closeTab(session.id);
            }
          }}
          onRowContextMenu={(event) => {
            event.preventDefault();
            openMenu(session.id);
          }}
          onChipKeyDown={(event) => handleChipKeyDown(session.id, event)}
          onClose={() => closeTab(session.id)}
        />
      )),
      ...toolTabs.map((tool) => (
        <MemoToolChip
          key={tool.id}
          tool={tool}
          selected={activeTabId === tool.id}
          multiselected={selection.has(tool.id)}
          tabIndex={tabIndexFor(tool.id)}
          browser={tool.kind === "browser" ? browserPages.get(tool.browserId) : undefined}
          tooltip={
            tool.kind === "browser"
              ? (browserPages.get(tool.browserId)?.url ?? tool.browserId)
              : tool.path
          }
          menuOpen={menuAnchorId === tool.id}
          onTabClick={(event) => handleTabClick({ id: tool.id }, event)}
          onTabAuxClick={(event) => {
            if (event.button === 1) {
              event.preventDefault();
              closeTab(tool.id);
            }
          }}
          onRowContextMenu={(event) => {
            event.preventDefault();
            openMenu(tool.id);
          }}
          onChipKeyDown={(event) => handleChipKeyDown(tool.id, event)}
          onClose={() => closeTab(tool.id)}
        />
      )),
    ],
    [
      chips,
      toolTabs,
      browserPages,
      activeTabId,
      selection,
      tabIndexFor,
      menuAnchorId,
      onTakeBack,
      handleTabClick,
      closeTab,
      openMenu,
      handleChipKeyDown,
    ],
  );

  return (
    <div className="workspace-session-tabs">
      {/* The row of tabs scrolls; the add button below it stays outside
          the scrollport, so a full strip cannot carry it off screen. */}
      <div
        className="workspace-session-tabs-scroll"
        role="tablist"
        aria-label="Tabs"
        ref={scrollportRef}
        data-fade-left={fade.left ? "true" : "false"}
        data-fade-right={fade.right ? "true" : "false"}
      >
        {rows}
      </div>
      <div className="workspace-session-add-wrap">
        <button
          ref={addButtonRef}
          type="button"
          className="workspace-session-add"
          onClick={newTab.onToggle}
          title="New tab"
          aria-label="New tab"
          aria-haspopup="menu"
          aria-expanded={newTab.open}
          disabled={newTab.creating}
        >
          +
        </button>
        <WorkspaceNewTabMenu
          open={newTab.open}
          triggerRef={addButtonRef}
          creating={newTab.creating}
          workspaceSelected={newTab.workspaceSelected}
          onAgent={newTab.onAgent}
          onTerminal={newTab.onTerminal}
          onBrowser={newTab.onBrowser}
          onClose={newTab.onCloseMenu}
        />
        {providerMenu}
      </div>
      <span className="workspace-tabs-spacer" />
      <button
        ref={overviewTriggerRef}
        type="button"
        className="workspace-rate"
        aria-haspopup="listbox"
        aria-expanded={overviewOpen}
        aria-label={`${statusText} — show all ${tabs.length} ${overviewTabsWord}${overviewUnopenedCount === 0 ? "" : ` and ${overviewUnopenedCount} more ${overviewSessionWord}`}${attentionSummary}`}
        onClick={() => {
          if (!overviewOpen) openOverview("press");
          else if (overviewSourceRef.current === "hover") overviewSourceRef.current = "press";
          else closeOverview();
        }}
        onMouseEnter={scheduleOverviewOpen}
        onMouseLeave={scheduleOverviewClose}
      >
        {unopenedAttentionCount > 0 ? (
          <span className="workspace-status-dot strip-dot-attention" aria-hidden="true" />
        ) : null}
        {statusText}
      </button>
      <SessionOverviewMenu
        key={overviewEpoch}
        open={overviewOpen}
        triggerRef={overviewTriggerRef}
        contentRef={overviewRootRef}
        sessions={overviewSessions}
        stripOrder={stripOrder}
        tabs={tabs}
        browserPages={browserPages}
        activeTabId={activeTabId}
        activeSessionId={selectedSessionId}
        workspaceName={workspaceName}
        onOpen={handleOverviewOpen}
        onSelectTab={handleOverviewSelectTool}
        onClose={closeOverview}
        onListEnter={cancelOverviewTimers}
        onListLeave={scheduleOverviewClose}
      />
      <div className="workspace-sr-only" role="status" aria-live="polite">
        {tabSelection.announcement}
      </div>
      <SessionTabMenu
        key={tabClose.menu?.anchorId ?? "closed"}
        open={tabClose.menu !== null}
        anchorRef={tabClose.anchorRef}
        entries={tabClose.menu?.entries ?? []}
        onEntry={tabClose.activateEntry}
        copyEntryValue={tabClose.copyEntryValue}
        onClose={tabClose.closeMenu}
      />
      <ConfirmDialog
        open={tabClose.confirm !== null}
        title={tabClose.confirm?.title ?? ""}
        message={tabClose.confirm?.message ?? ""}
        confirmLabel={tabClose.confirm?.confirmLabel ?? ""}
        tone={tabClose.confirm?.tone ?? "danger"}
        onConfirm={tabClose.confirmClose}
        onCancel={tabClose.cancelClose}
      />
    </div>
  );
}
