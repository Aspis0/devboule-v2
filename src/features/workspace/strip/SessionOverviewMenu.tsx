import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
} from "react";
import type { Session } from "../../../types/ipc";
import { relativeTime } from "../../../lib/relativeTime";
import { isImeComposition } from "../../../lib/imeComposition";
import { useMenuOpen } from "../../../lib/menuOpen";
import { AnchoredPopover } from "../popoverPlace";
import { sessionKindWord, sessionProviderLabel, sessionTitle } from "../workspaceSessions";
import { chipDisplay } from "./stripDisplay";
import { sessionAttentionLabel } from "../sessionAttention";
import { sessionLastActiveMs, sessionStartedLabel } from "./sessionOverview";
import { composeOverviewGroups, type OverviewRow } from "./overviewTabs";
import {
  toolTabDirectory,
  toolTabKindLabel,
  toolTabLabel,
  toolTabSubject,
  type StripTab,
  type ToolTab,
} from "./toolTabs";
import { StripKindMark } from "./StripKindMark";
import { DOT_CLASS, type BrowserTabPage } from "./StripChip";

interface SessionOverviewMenuProps {
  /** The menu is up; the owner owns the open state and says so. */
  open: boolean;
  /** The count button the menu hangs from. */
  triggerRef: RefObject<HTMLButtonElement | null>;
  /** The popover root, owned here but read by the owner for focus restore. */
  contentRef: RefObject<HTMLDivElement | null>;
  /** Every roster session of the selected workspace, unordered. */
  sessions: readonly Session[];
  /** The strip's session ids in strip order: membership here marks a row open. */
  stripOrder: readonly string[];
  /** Every tab of the selected workspace in strip order: session tabs and
   * tool tabs. Workspace scopes this; the menu lists exactly what it gets. */
  tabs: readonly StripTab[];
  /** What each browser tab's page is called, by browser id. A page names
   * itself here; without it the row would read as its own id. */
  browserPages: ReadonlyMap<string, BrowserTabPage>;
  /** The strip's active tab id: seeds the preview when it names a row. */
  activeTabId: string | null;
  activeSessionId: string | null;
  /** The selected workspace's title; null renders no workspace line. */
  workspaceName: string | null;
  onOpen: (sessionId: string) => void;
  /** A tool row's click or Enter: the owner selects the tab and closes. */
  onSelectTab: (id: string) => void;
  onClose: () => void;
  onListEnter: () => void;
  onListLeave: () => void;
}

export function SessionOverviewMenu({
  open,
  triggerRef,
  contentRef,
  sessions,
  stripOrder,
  tabs,
  browserPages,
  activeTabId,
  activeSessionId,
  workspaceName,
  onOpen,
  onSelectTab,
  onClose,
  onListEnter,
  onListLeave,
}: SessionOverviewMenuProps) {
  useMenuOpen(open, onClose);

  // One clock for every row and the preview, seeded at mount — the owner
  // remounts the menu per opening, so mount time is open time — and
  // ticking while the menu stands open, the History panel's arrangement.
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!open) return;
    const id = window.setInterval(() => {
      setNow(Date.now());
    }, 30_000);
    return () => window.clearInterval(id);
  }, [open]);

  const groups = useMemo(
    () => composeOverviewGroups(tabs, sessions, stripOrder),
    [tabs, sessions, stripOrder],
  );
  const rows = useMemo(() => groups.flatMap((group) => group.rows), [groups]);
  const seedId = rows.some((row) => row.id === activeTabId)
    ? activeTabId
    : rows.some((row) => row.id === activeSessionId)
      ? activeSessionId
      : (rows[0]?.id ?? null);
  // Hovering or keyboarding a row previews it; otherwise the seed row —
  // the preview never sits empty. Seeded in the initializers: the owner
  // remounts per opening, and a roster push must move neither behind a
  // live pointer.
  const [previewId, setPreviewId] = useState<string | null>(() => seedId);
  const [focusedId, setFocusedId] = useState<string | null>(() => seedId);

  const focusOption = (id: string) => {
    contentRef.current
      ?.querySelector<HTMLElement>(`[data-overview-option="${CSS.escape(id)}"]`)
      ?.focus({ preventScroll: true });
  };

  // Once per open, and only when the trigger holds focus — a hover-open
  // never steals the pane's focus. No state is set, so this stays an effect.
  const openedRef = useRef(false);
  useEffect(() => {
    if (!open) {
      openedRef.current = false;
      return;
    }
    if (openedRef.current) return;
    openedRef.current = true;
    if (triggerRef.current?.contains(document.activeElement) === true && seedId !== null) {
      focusOption(seedId);
    }
  });

  // While open: an outside press, a resize, and Escape anywhere all close
  // through the owner's close, which restores focus only when it was
  // inside. Hover opens leave focus outside the portal, so the list's own
  // Escape never fires there — this listener closes those too. An IME
  // owns Escape mid-composition, never the menu.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!(event.target instanceof Node)) return;
      if (contentRef.current?.contains(event.target)) return;
      if (triggerRef.current?.contains(event.target)) return;
      onClose();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || isImeComposition(event)) return;
      onClose();
    };
    const onResize = () => onClose();
    window.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", onResize);
    };
  }, [contentRef, onClose, open, triggerRef]);

  if (!open) return null;

  const currentId = rows.some((row) => row.id === focusedId) ? focusedId : (rows[0]?.id ?? null);
  const preview =
    rows.find((row) => row.id === previewId) ??
    rows.find((row) => row.id === activeSessionId) ??
    rows[0] ??
    null;

  const step = (from: string | null, delta: 1 | -1): string | null => {
    if (rows.length === 0) return null;
    const index = rows.findIndex((row) => row.id === from);
    if (index === -1) return rows[delta === 1 ? 0 : rows.length - 1]?.id ?? null;
    return rows[(index + delta + rows.length) % rows.length]?.id ?? null;
  };

  const onListKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    // The IME owns every key it can hold mid-composition — arrows and Tab
    // included — so the list takes none of them from it. Escape travels
    // the document listener instead, which guards the same way.
    if (isImeComposition(event.nativeEvent)) return;
    if (event.key === "Tab") {
      // A body portal: letting Tab continue from here would resume at the
      // end of document.body. The owner's close hands focus back.
      event.preventDefault();
      onClose();
      return;
    }
    let next: string | null = null;
    if (event.key === "ArrowDown") next = step(currentId, 1);
    else if (event.key === "ArrowUp") next = step(currentId, -1);
    else if (event.key === "Home") next = rows[0]?.id ?? null;
    else if (event.key === "End") next = rows[rows.length - 1]?.id ?? null;
    else return;
    if (next === null) return;
    event.preventDefault();
    setFocusedId(next);
    setPreviewId(next);
    focusOption(next);
  };

  const onOptionKeyDown = (row: OverviewRow, event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (row.kind === "tool") onSelectTab(row.id);
      else onOpen(row.id);
    }
  };

  return (
    <AnchoredPopover
      containerRef={contentRef}
      anchorRef={triggerRef}
      onDismiss={onClose}
      className="workspace-surface-menu workspace-overview"
      onMouseEnter={onListEnter}
      onMouseLeave={onListLeave}
    >
      {rows.length === 0 ? (
        <div className="workspace-overview-empty">No tabs in this workspace.</div>
      ) : (
        <div
          className="workspace-overview-list"
          role="listbox"
          aria-label="All tabs"
          onKeyDown={onListKeyDown}
        >
          {groups.map((group) => (
            <div key={group.key} role="group" aria-label={group.label}>
              <div className="workspace-overview-group-label" aria-hidden="true">
                {group.label}
              </div>
              {group.rows.map((row) => {
                if (row.kind === "tool") {
                  const page =
                    row.tool.kind === "browser" ? browserPages.get(row.tool.browserId) : undefined;
                  const label = page?.label ?? toolTabLabel(row.tool);
                  const directory = toolTabDirectory(row.tool);
                  return (
                    <div
                      key={row.id}
                      role="option"
                      tabIndex={row.id === currentId ? 0 : -1}
                      aria-selected={row.id === activeTabId}
                      aria-label={`${label}${directory === null ? "" : `, ${directory}`}, ${row.tool.kind} tab, open tab`}
                      data-overview-option={row.id}
                      className="workspace-overview-option"
                      onClick={() => onSelectTab(row.id)}
                      onMouseEnter={() => setPreviewId(row.id)}
                      onFocus={() => {
                        setFocusedId(row.id);
                        setPreviewId(row.id);
                      }}
                      onKeyDown={(event) => onOptionKeyDown(row, event)}
                    >
                      <StripKindMark kind={row.tool.kind} />
                      <span className="workspace-overview-title">{label}</span>
                      <span className="workspace-overview-open">Open</span>
                    </div>
                  );
                }
                const session = row.session;
                const display = chipDisplay(session);
                const title = sessionTitle(session);
                const attentionLabel = sessionAttentionLabel(session);
                const lastActive = sessionLastActiveMs(session, now);
                return (
                  <div
                    key={session.id}
                    role="option"
                    tabIndex={session.id === currentId ? 0 : -1}
                    aria-selected={session.id === activeTabId}
                    aria-label={`${title}, ${sessionProviderLabel(session)}, ${display.stateLine}${attentionLabel === null ? "" : `, ${attentionLabel}`}${row.open ? ", open tab" : ""}`}
                    data-overview-option={session.id}
                    className="workspace-overview-option"
                    onClick={() => onOpen(session.id)}
                    onMouseEnter={() => setPreviewId(session.id)}
                    onFocus={() => {
                      setFocusedId(session.id);
                      setPreviewId(session.id);
                    }}
                    onKeyDown={(event) => onOptionKeyDown(row, event)}
                  >
                    <span
                      className={`workspace-status-dot ${DOT_CLASS[display.dot]}${display.pulse ? " dot-pulse" : ""}`}
                    />
                    <StripKindMark kind={session.kind} />
                    <span className="workspace-overview-title">{title}</span>
                    {attentionLabel !== null ? (
                      <span className="workspace-overview-attention">{attentionLabel}</span>
                    ) : null}
                    {row.open ? <span className="workspace-overview-open">Open</span> : null}
                    {lastActive === null ? null : (
                      <span className="workspace-overview-time">
                        {relativeTime(lastActive, now)}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      )}
      {preview !== null ? (
        preview.kind === "tool" ? (
          <ToolOverviewPreview
            tool={preview.tool}
            page={
              preview.tool.kind === "browser" ? browserPages.get(preview.tool.browserId) : undefined
            }
          />
        ) : (
          <OverviewPreview session={preview.session} workspaceName={workspaceName} now={now} />
        )
      ) : null}
    </AnchoredPopover>
  );
}

function ToolOverviewPreview({ tool, page }: { tool: ToolTab; page?: BrowserTabPage }) {
  return (
    <div className="workspace-overview-preview">
      <div className="workspace-overview-preview-title">{page?.label ?? toolTabLabel(tool)}</div>
      <div className="workspace-overview-preview-state">{toolTabKindLabel(tool)}</div>
      <div className="workspace-overview-preview-meta">{page?.url ?? toolTabSubject(tool)}</div>
    </div>
  );
}

function OverviewPreview({
  session,
  workspaceName,
  now,
}: {
  session: Session;
  workspaceName: string | null;
  now: number;
}) {
  const display = chipDisplay(session);
  const lastActive = sessionLastActiveMs(session, now);
  // Creation is not activity: a row with no last-activity fact says when
  // it started, never "active", and a row with neither stamp says nothing.
  const started = sessionStartedLabel(session.createdAtMs, now);
  let activityLine: string | null = null;
  if (lastActive !== null) activityLine = `active ${relativeTime(lastActive, now)}`;
  else if (started !== null) activityLine = `started ${started}`;
  const workspaceLine = session.workspaceId === null ? "No workspace" : (workspaceName ?? null);
  const meta = [
    session.provider ?? sessionKindWord(session.kind),
    workspaceLine,
    activityLine,
  ].filter((line) => line !== null);
  const goal = session.goal?.trim() || null;
  return (
    <div className="workspace-overview-preview">
      <div className="workspace-overview-preview-title">{sessionTitle(session)}</div>
      <div className="workspace-overview-preview-state">{display.stateLine}</div>
      {display.detailLines.length > 0 ? (
        <div className="workspace-overview-preview-state">{display.detailLines.join(" ")}</div>
      ) : null}
      <div className="workspace-overview-preview-meta">{meta.join(" · ")}</div>
      {goal !== null ? <div className="workspace-overview-preview-goal">{goal}</div> : null}
    </div>
  );
}
