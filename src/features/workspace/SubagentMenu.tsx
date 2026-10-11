// The one "who is working" line above the composer and the list it opens. The
// list renders through the shared anchored portal because the line sits in the
// composer, where the panel's overflow would clip what opens from it. Which side
// the list opens on is the portal's placement call.
import { useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { AnchoredPopover } from "./popoverPlace";
import { useMenuOpen } from "../../lib/menuOpen";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import { useConfirmAsk } from "../../components/ConfirmHost";
import "./SubagentMenu.css";
import type { SessionTask } from "../../types/ipc";
import type { AgentSubagentStatus } from "../../lib/agentSession";
import { isArchivable, type SubagentRow } from "./subagentRows";
import type { ArchiveTarget } from "./subagentArchive";
import { StripKindMark } from "./strip/StripKindMark";
import { useOpenedSessions } from "./openedSessions";
import { useTaskClock } from "./useTaskClock";
import {
  rowClock,
  subagentTitle,
  taskClocks,
  whoIsWorking,
  type LineSegment,
} from "./whoIsWorkingLine";

/** A row paints its state, except a child waiting on a person paints the approval. */
type SubagentDot = AgentSubagentStatus | "attention";

function subagentDotClass(dot: SubagentDot): string {
  return `workspace-subagent-status-${dot}`;
}

// Every clause is a checked effect of the close: rows and tabs leave with the
// roster, the transcript stays in History, and attachments do not survive it.
const ARCHIVE_ASK_MESSAGE =
  "They leave this list and their tabs close. Their transcripts stay in History; their attached files are removed.";
const STOP_MESSAGE = "The agent stops where it is. Its transcript stays in History.";

export interface SubagentMenuProps {
  rows: readonly SubagentRow[];
  /** The open agent's task list: its running commands and the start of each running turn. */
  tasks: readonly SessionTask[];
  onOpenSession?: (sessionId: string) => void;
  attentionById?: ReadonlyMap<string, string>;
  /** Closes the named children, reads the roster back, and answers with one
   * plain sentence for each child that did not close — never raw daemon text. */
  onArchiveFinished?: (targets: readonly ArchiveTarget[]) => Promise<ReadonlyMap<string, string>>;
  /** Stops a running child; answers with the refusal sentence, or null. */
  onStopSubagent?: (sessionId: string) => Promise<string | null>;
  /** Opens the Tasks tab: the line's road when it names commands only. */
  onOpenTasks?: () => void;
  /** Runs when the line empties while focus was in it, so focus has somewhere to land. */
  onEmptied?: () => void;
}

function segmentText(segment: LineSegment): string {
  const parts = [segment.name, segment.word];
  if (segment.tone === "live") {
    if (segment.clock !== null) parts.push(segment.clock);
    if (segment.commands !== null) parts.push(segment.commands);
  }
  return parts.join(" ");
}

export function SubagentMenu({
  rows,
  tasks,
  onOpenSession,
  attentionById,
  onArchiveFinished,
  onStopSubagent,
  onOpenTasks,
  onEmptied,
}: SubagentMenuProps) {
  const lineRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const sentencePrefix = useId();
  const askConfirm = useConfirmAsk();
  const [open, setOpen] = useState(false);
  const [sentences, setSentences] = useState<ReadonlyMap<string, string>>(new Map());
  const [archiving, setArchiving] = useState(false);
  const [archiveNonce, setArchiveNonce] = useState(0);
  /** The children the open ask was raised for; null while no ask is up. */
  const [ask, setAsk] = useState<readonly ArchiveTarget[] | null>(null);
  /** Same-tick re-entry: the dialog's open state clears only on the next render. */
  const runningRef = useRef(false);
  const opened = useOpenedSessions();

  const live =
    rows.some((row) => row.status === "running") || tasks.some((task) => task.state === "running");
  const now = useTaskClock(live);
  const clocks = useMemo(() => taskClocks(tasks), [tasks]);
  const waitingIds = useMemo(() => new Set(attentionById?.keys()), [attentionById]);
  const { segments, commandsOnly } = useMemo(
    () => whoIsWorking({ rows, waitingIds, seenFailedIds: opened, clocks, now }),
    [rows, waitingIds, opened, clocks, now],
  );

  const showing = segments.length > 0;
  const wasShowing = useRef(showing);
  useEffect(() => {
    const was = wasShowing.current;
    wasShowing.current = showing;
    if (!was || showing || onEmptied === undefined) return;
    // The line's own element is gone by now: focus that fell to the body, or to
    // an element the removal detached, has nowhere to stay.
    const focused = document.activeElement;
    if (focused === null || focused === document.body || !focused.isConnected) onEmptied();
  }, [showing, onEmptied]);

  const close = useCallback(() => {
    if (menuRef.current?.contains(document.activeElement)) {
      lineRef.current?.focus({ preventScroll: true });
    }
    setOpen(false);
  }, []);

  useMenuOpen(open, close);

  useEffect(() => {
    // The ask owns Escape and the press outside itself while it is up:
    // closing the list under the ask unmounts the button Cancel returns to.
    if (!open || ask !== null) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    // Outside press closes on pointerdown, the house pattern: the press
    // that opens the list is a click, and the line's own click toggles.
    const closeOnOutsidePress = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuRef.current?.contains(target)) return;
      if (lineRef.current?.contains(target)) return;
      close();
    };
    document.addEventListener("keydown", closeOnEscape);
    window.addEventListener("pointerdown", closeOnOutsidePress);
    return () => {
      document.removeEventListener("keydown", closeOnEscape);
      window.removeEventListener("pointerdown", closeOnOutsidePress);
    };
  }, [open, ask, close]);

  // After an archive the focus lands on the first remaining row, else back on
  // the line; when the last row goes, the surface owns where focus lands.
  useEffect(() => {
    if (archiveNonce === 0) return;
    const firstRow = menuRef.current?.querySelector<HTMLButtonElement>(
      ".workspace-subagent-row:not([disabled])",
    );
    if (firstRow) {
      firstRow.focus({ preventScroll: true });
      return;
    }
    lineRef.current?.focus({ preventScroll: true });
    close();
  }, [archiveNonce, close]);

  // The dialog gives focus back to the action the ask came from; when the
  // roster dropped the last target meanwhile, that action is gone and the line
  // is the nearest control left. The dialog's own effect has already run.
  const askWasUpRef = useRef(false);
  useEffect(() => {
    const wasUp = askWasUpRef.current;
    askWasUpRef.current = ask !== null;
    if (!wasUp || ask !== null) return;
    if (document.activeElement !== null && document.activeElement !== document.body) return;
    lineRef.current?.focus({ preventScroll: true });
  }, [ask]);

  if (segments.length === 0) return null;

  const archivable = rows.filter(isArchivable);

  // Only a created child is a session. A task id that matches a roster id
  // names nothing of the task's, so it neither opens nor borrows that
  // session's approval ask.
  const openable = (row: SubagentRow): boolean => row.kind === "child";
  const attentionOf = (row: SubagentRow): string | undefined =>
    openable(row) ? attentionById?.get(row.id) : undefined;

  // The generation each child had as the ask took it — the row's identity at
  // the moment the user saw the count.
  const askTarget = (row: SubagentRow): ArchiveTarget => ({
    id: row.id,
    generation: row.generation,
  });

  const archiveFinished = async (targets: readonly ArchiveTarget[]): Promise<void> => {
    if (runningRef.current || onArchiveFinished === undefined) return;
    runningRef.current = true;
    setArchiving(true);
    try {
      setSentences(await onArchiveFinished(targets));
    } finally {
      runningRef.current = false;
      setArchiving(false);
      setArchiveNonce((nonce) => nonce + 1);
    }
  };

  const openRow = (row: SubagentRow) => {
    close();
    onOpenSession?.(row.id);
  };

  const stopRow = async (row: SubagentRow): Promise<void> => {
    if (onStopSubagent === undefined) return;
    const confirmed = await askConfirm({
      title: `Stop ${subagentTitle(row.title, row.id)}?`,
      message: STOP_MESSAGE,
      confirmLabel: "Stop",
      cancelLabel: "Keep it running",
    });
    if (!confirmed) return;
    const refusal = await onStopSubagent(row.id);
    setSentences((current) => {
      const next = new Map(current);
      if (refusal === null) next.delete(row.id);
      else next.set(row.id, refusal);
      return next;
    });
  };

  const label = segments.map(segmentText).join(", ");

  return (
    <div className="workspace-subagent-menu">
      <button
        type="button"
        ref={lineRef}
        className="workspace-who-line"
        data-testid="who-is-working"
        aria-label={label}
        aria-expanded={commandsOnly ? undefined : open}
        aria-controls={open ? listId : undefined}
        onClick={() => (commandsOnly ? onOpenTasks?.() : setOpen((value) => !value))}
      >
        {segments.map((segment) => (
          <span className="workspace-who-segment" key={`${segment.tone}:${segment.name}`}>
            <span
              className={`workspace-who-dot workspace-who-dot-${segment.tone}`}
              aria-hidden="true"
            />
            <span className="workspace-who-name">{segment.name}</span>
            {segment.tone === "attention" ? (
              <span className="workspace-who-word workspace-who-word-attention">
                {segment.word}
              </span>
            ) : (
              <span className="workspace-who-word">{segment.word}</span>
            )}
            {segment.tone === "live" && segment.clock !== null ? (
              <span className="workspace-who-clock">{segment.clock}</span>
            ) : null}
            {segment.tone === "live" && segment.commands !== null ? (
              <span className="workspace-who-word">· {segment.commands}</span>
            ) : null}
          </span>
        ))}
        {commandsOnly ? null : (
          <svg
            className={`workspace-who-chevron${open ? " is-open" : ""}`}
            width={12}
            height={12}
            viewBox="0 0 24 24"
            aria-hidden="true"
            focusable="false"
          >
            <path d="m18 15-6-6-6 6" />
          </svg>
        )}
      </button>
      {open ? (
        <AnchoredPopover
          anchorRef={lineRef}
          containerRef={menuRef}
          onDismiss={close}
          openAbove
          className="workspace-subagent-list"
          id={listId}
        >
          <div className="workspace-subagent-list-head">
            Agents
            {onArchiveFinished !== undefined && archivable.length > 0 ? (
              <button
                type="button"
                className="workspace-subagent-action"
                data-testid="subagent-archive"
                disabled={archiving}
                onClick={() => setAsk(archivable.map(askTarget))}
              >
                Archive {archivable.length} finished
              </button>
            ) : null}
          </div>
          <div role="list">
            {rows.map((row) => {
              const sentence = row.kind === "child" ? sentences.get(row.id) : undefined;
              const attention = attentionOf(row);
              const title = subagentTitle(row.title, row.id);
              const clock = rowClock(clocks, row, now);
              const canStop =
                row.kind === "child" &&
                row.status === "running" &&
                attention === undefined &&
                onStopSubagent !== undefined;
              return (
                <div key={`${row.kind}:${row.id}`} role="listitem">
                  <div className="workspace-subagent-row-line">
                    <button
                      type="button"
                      className="workspace-subagent-row"
                      disabled={!openable(row) || onOpenSession === undefined}
                      aria-label={`${title}, ${row.status}${attention !== undefined ? `, ${attention}` : ""}, ${openable(row) && onOpenSession !== undefined ? "Open in tab" : "Session unavailable"}`}
                      aria-describedby={
                        sentence !== undefined ? `${sentencePrefix}${row.id}` : undefined
                      }
                      title={!openable(row) ? "Session unavailable" : undefined}
                      onClick={() => openRow(row)}
                    >
                      {row.kind === "child" && row.provider !== undefined ? (
                        <span className="workspace-subagent-lane">
                          <StripKindMark kind={row.provider} />
                        </span>
                      ) : null}
                      <span className="workspace-subagent-row-title" title={title}>
                        {title}
                      </span>
                      {openable(row) ? null : <span>Unavailable</span>}
                      <span className="workspace-subagent-row-state">
                        <span
                          className={`workspace-subagent-status-dot ${subagentDotClass(attention !== undefined ? "attention" : row.status)}`}
                          aria-hidden="true"
                        />
                        {attention !== undefined ? (
                          <span className="workspace-subagent-attention">needs you</span>
                        ) : row.status === "running" && clock !== null ? (
                          <span className="workspace-subagent-clock">{clock}</span>
                        ) : (
                          <span>{row.status}</span>
                        )}
                      </span>
                    </button>
                    {canStop ? (
                      <button
                        type="button"
                        className="workspace-subagent-stop"
                        onClick={() => void stopRow(row)}
                      >
                        Stop
                      </button>
                    ) : null}
                  </div>
                  {sentence !== undefined ? (
                    <p className="workspace-subagent-row-failure" id={`${sentencePrefix}${row.id}`}>
                      {sentence}
                    </p>
                  ) : null}
                </div>
              );
            })}
          </div>
        </AnchoredPopover>
      ) : null}
      <ConfirmDialog
        open={ask !== null}
        title={
          ask === null
            ? ""
            : `Archive ${ask.length} finished subagent${ask.length === 1 ? "" : "s"}?`
        }
        message={ARCHIVE_ASK_MESSAGE}
        confirmLabel="Archive"
        tone="danger"
        onConfirm={() => {
          const targets = ask;
          setAsk(null);
          if (targets !== null && targets.length > 0) void archiveFinished(targets);
        }}
        onCancel={() => setAsk(null)}
      />
    </div>
  );
}
