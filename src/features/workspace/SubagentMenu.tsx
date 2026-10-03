// The subagent pill in the pane header and the menu it opens. The menu
// renders through the shared anchored portal because the pill sits at the
// top of the centre panel, where the panel's overflow would clip whatever
// opens from it. Which side the menu opens on is the portal's placement
// call: above when the content fits above, below over the transcript when
// it does not.
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { AnchoredPopover } from "./popoverPlace";
import { useMenuOpen } from "../../lib/menuOpen";
import { ConfirmDialog } from "../../components/ConfirmDialog";
import "./SubagentMenu.css";
import type { AgentSubagentStatus } from "../../lib/agentSession";
import { countSubagentStatuses, isArchivable, type SubagentRow } from "./subagentRows";

function shortSubagentId(id: string): string {
  return id.length > 16 ? `${id.slice(0, 12)}…` : id;
}

function subagentTitle(title: string | null, id: string): string {
  return title?.trim() ? title : shortSubagentId(id);
}

function subagentDotClass(status: AgentSubagentStatus): string {
  return `workspace-subagent-status-${status}`;
}

// Every clause is a checked effect of the close: rows and tabs leave with the
// roster, the transcript stays in History, and attachments do not survive it.
const ARCHIVE_ASK_MESSAGE =
  "They leave this list and their tabs close. Their transcripts stay in History; their attached files are removed.";

/** One child as the ask takes it: its id and the generation its roster row
 * held then; the act closes it only while that still matches. */
export interface SubagentArchiveTarget {
  id: string;
  generation: number | null;
}

export interface SubagentMenuProps {
  rows: readonly SubagentRow[];
  onOpenSession?: (sessionId: string) => void;
  attentionById?: ReadonlyMap<string, string>;
  /** Closes the named children, reads the roster back, and answers with one
   * plain sentence for each child that did not close — never raw daemon text. */
  onArchiveFinished?: (
    targets: readonly SubagentArchiveTarget[],
  ) => Promise<ReadonlyMap<string, string>>;
}

export function SubagentMenu({
  rows,
  onOpenSession,
  attentionById,
  onArchiveFinished,
}: SubagentMenuProps) {
  const pillRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const sentencePrefix = useId();
  const [open, setOpen] = useState(false);
  const [sentences, setSentences] = useState<ReadonlyMap<string, string>>(new Map());
  const [archiving, setArchiving] = useState(false);
  const [archiveNonce, setArchiveNonce] = useState(0);
  /** The children the open ask was raised for; null while no ask is up. */
  const [ask, setAsk] = useState<readonly SubagentArchiveTarget[] | null>(null);
  /** Same-tick re-entry: the dialog's open state clears only on the next render. */
  const runningRef = useRef(false);

  const close = useCallback(() => {
    if (menuRef.current?.contains(document.activeElement)) {
      pillRef.current?.focus({ preventScroll: true });
    }
    setOpen(false);
  }, []);

  useMenuOpen(open, close);

  useEffect(() => {
    // The ask owns Escape and the press outside itself while it is up:
    // closing the menu under the ask unmounts the button Cancel returns to.
    if (!open || ask !== null) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
    };
    // Outside press closes on pointerdown, the house pattern: the press
    // that opens the menu is a click, and the pill's own click toggles.
    const closeOnOutsidePress = (event: PointerEvent) => {
      const target = event.target;
      if (!(target instanceof Node)) return;
      if (menuRef.current?.contains(target)) return;
      if (pillRef.current?.contains(target)) return;
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
  // the pill; when the last row goes, the surface owns where focus lands.
  useEffect(() => {
    if (archiveNonce === 0) return;
    const firstRow = menuRef.current?.querySelector<HTMLButtonElement>(
      ".workspace-subagent-row:not([disabled])",
    );
    if (firstRow) {
      firstRow.focus({ preventScroll: true });
      return;
    }
    pillRef.current?.focus({ preventScroll: true });
    close();
  }, [archiveNonce, close]);

  // The dialog gives focus back to the action the ask came from; when the
  // roster dropped the last target meanwhile, that action is gone and the pill
  // is the nearest control left. The dialog's own effect has already run.
  const askWasUpRef = useRef(false);
  useEffect(() => {
    const wasUp = askWasUpRef.current;
    askWasUpRef.current = ask !== null;
    if (!wasUp || ask !== null) return;
    if (document.activeElement !== null && document.activeElement !== document.body) return;
    pillRef.current?.focus({ preventScroll: true });
  }, [ask]);

  if (rows.length === 0) return null;

  const archivable = rows.filter(isArchivable);

  // Only a created child is a session. A task id that matches a roster id
  // names nothing of the task's, so it neither opens nor borrows that
  // session's approval ask.
  const openable = (row: SubagentRow): boolean => row.kind === "child";
  const attentionOf = (row: SubagentRow): string | undefined =>
    openable(row) ? attentionById?.get(row.id) : undefined;

  // The generation each child had as the ask took it — the row's identity at
  // the moment the user saw the count.
  const askTarget = (row: SubagentRow): SubagentArchiveTarget => ({
    id: row.id,
    generation: row.generation,
  });

  const archiveFinished = async (targets: readonly SubagentArchiveTarget[]): Promise<void> => {
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

  const { failed, running: working } = countSubagentStatuses(rows);
  const attentionCount = rows.filter((row) => attentionOf(row) !== undefined).length;
  const counts = [
    ...(attentionCount > 0
      ? [`${attentionCount} ${attentionCount === 1 ? "needs" : "need"} your approval`]
      : []),
    ...(failed > 0 ? [`${failed} failed`] : []),
    ...(working > 0 ? [`${working} working`] : []),
  ].join(", ");
  // A settled fan-out has no colour worth showing, so the pill names what
  // its chevron opens instead of going wordless.
  const totalLabel = `${rows.length} subagent${rows.length === 1 ? "" : "s"}`;
  const pillLabel = counts === "" ? totalLabel : `Subagents: ${counts}`;

  return (
    <div className="workspace-subagent-menu">
      <button
        type="button"
        ref={pillRef}
        className="workspace-subagent-pill"
        data-testid="subagent-pill"
        aria-label={pillLabel}
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        onClick={() => setOpen((value) => !value)}
      >
        {attentionCount > 0 ? (
          <span className="workspace-subagent-pill-group workspace-subagent-attention">
            <span
              className="workspace-subagent-status-dot workspace-subagent-status-failed"
              aria-hidden="true"
            />
            <span>
              {attentionCount} {attentionCount === 1 ? "needs" : "need"} your approval
            </span>
          </span>
        ) : null}
        {failed > 0 ? (
          <span className="workspace-subagent-pill-group">
            <span
              className={`workspace-subagent-status-dot ${subagentDotClass("failed")}`}
              aria-hidden="true"
            />
            <span>{failed} failed</span>
          </span>
        ) : null}
        {working > 0 ? (
          <span className="workspace-subagent-pill-group">
            <span
              className={`workspace-subagent-status-dot ${subagentDotClass("running")} dot-pulse`}
              aria-hidden="true"
            />
            <span>{working} working</span>
          </span>
        ) : null}
        {counts === "" ? <span className="workspace-subagent-pill-group">{totalLabel}</span> : null}
        <svg
          className="workspace-subagent-pill-chevron"
          width={12}
          height={12}
          viewBox="0 0 24 24"
          aria-hidden="true"
          focusable="false"
        >
          <path d="m18 15-6-6-6 6" />
        </svg>
      </button>
      {open ? (
        <AnchoredPopover
          anchorRef={pillRef}
          containerRef={menuRef}
          onDismiss={close}
          openAbove
          className="workspace-subagent-list"
          id={listId}
        >
          <div className="workspace-subagent-list-head">
            Subagents
            {onArchiveFinished !== undefined && archivable.length > 0 ? (
              <button
                type="button"
                className="workspace-subagent-action"
                data-testid="subagent-archive"
                disabled={archiving}
                onClick={() => setAsk(archivable.map(askTarget))}
              >
                Archive {archivable.length} finished subagent
                {archivable.length === 1 ? "" : "s"}
              </button>
            ) : null}
          </div>
          <div role="list">
            {rows.map((row) => {
              const sentence = row.kind === "child" ? sentences.get(row.id) : undefined;
              const attention = attentionOf(row);
              const title = subagentTitle(row.title, row.id);
              return (
                <div key={`${row.kind}:${row.id}`} role="listitem">
                  <button
                    type="button"
                    className="workspace-subagent-row"
                    disabled={!openable(row) || onOpenSession === undefined}
                    aria-label={`${title}, ${row.status}${attention !== undefined ? `, ${attention}` : ""}, ${openable(row) && onOpenSession !== undefined ? "Open in tab" : "Session unavailable"}`}
                    aria-describedby={
                      sentence !== undefined ? `${sentencePrefix}${row.id}` : undefined
                    }
                    title={!openable(row) ? "Session unavailable" : undefined}
                    onClick={() => {
                      close();
                      onOpenSession?.(row.id);
                    }}
                  >
                    <span
                      className={`workspace-subagent-status-dot ${subagentDotClass(row.status)}`}
                      aria-hidden="true"
                    />
                    <span className="workspace-subagent-row-title" title={title}>
                      {title}
                    </span>
                    {attention !== undefined ? (
                      <span className="workspace-subagent-attention">{attention}</span>
                    ) : null}
                    {!openable(row) ? <span>Unavailable</span> : null}
                    <svg
                      className="workspace-subagent-row-chevron"
                      width={12}
                      height={12}
                      viewBox="0 0 24 24"
                      aria-hidden="true"
                      focusable="false"
                    >
                      <path d="m9 18 6-6-6-6" />
                    </svg>
                  </button>
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
