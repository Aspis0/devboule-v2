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
import type {
  AgentSubagent,
  AgentSubagentStatus,
  AgentSubagentStatusCounts,
} from "../../lib/agentSession";
import type { SessionState } from "../../types/ipc";

function shortSubagentId(id: string): string {
  return id.length > 16 ? `${id.slice(0, 12)}…` : id;
}

function subagentTitle(title: string | null, id: string): string {
  return title?.trim() ? title : shortSubagentId(id);
}

function subagentDotClass(status: AgentSubagentStatus): string {
  return `workspace-subagent-status-${status}`;
}

// Paseo's finished set for the same action: completed | failed | canceled.
const FINISHED_SUBAGENT_STATUSES: ReadonlySet<AgentSubagentStatus> = new Set([
  "finished",
  "failed",
  "stopped",
]);

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
  subagents: AgentSubagent[];
  statusCounts: AgentSubagentStatusCounts;
  onOpenSession?: (sessionId: string) => void;
  sessionIds?: ReadonlySet<string>;
  attentionById?: ReadonlyMap<string, string>;
  onRefreshSessions?: () => Promise<void>;
  /** The roster rows the ask reads each child's generation from. */
  sessionRoster?: ReadonlyArray<{ id: string; state?: SessionState }>;
  /** Closes the named children, reads the roster back, and answers with one
   * plain sentence for each child that did not close — never raw daemon text. */
  onArchiveFinished?: (
    targets: readonly SubagentArchiveTarget[],
  ) => Promise<ReadonlyMap<string, string>>;
}

export function SubagentMenu({
  subagents,
  statusCounts,
  onOpenSession,
  sessionIds,
  attentionById,
  onRefreshSessions,
  sessionRoster,
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
  // the pill; when the last child goes, the surface moves focus before we unmount.
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

  if (subagents.length === 0) return null;

  // Archivable = finished AND listed by the roster: a task the provider runs
  // without a session of its own has nothing the archive could close.
  const archivable = subagents.filter(
    (row) => FINISHED_SUBAGENT_STATUSES.has(row.status) && sessionIds?.has(row.id) === true,
  );

  // The generation each child had as the ask took it — the row's identity at
  // the moment the user saw the count.
  const askTarget = (row: AgentSubagent): SubagentArchiveTarget => ({
    id: row.id,
    generation: sessionRoster?.find((session) => session.id === row.id)?.state?.generation ?? null,
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

  const failed = statusCounts.failed;
  const working = statusCounts.running;
  const attentionCount = subagents.filter((row) => attentionById?.has(row.id)).length;
  const counts = [
    ...(attentionCount > 0
      ? [`${attentionCount} ${attentionCount === 1 ? "needs" : "need"} your approval`]
      : []),
    ...(failed > 0 ? [`${failed} failed`] : []),
    ...(working > 0 ? [`${working} working`] : []),
  ].join(", ");
  const pillLabel = counts === "" ? "Subagents" : `Subagents: ${counts}`;

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
        onClick={() => {
          if (!open && subagents.some((row) => !sessionIds?.has(row.id)))
            void onRefreshSessions?.();
          setOpen((value) => !value);
        }}
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
            {subagents.map((subagent) => {
              const sentence = sentences.get(subagent.id);
              return (
                <div key={subagent.id} role="listitem">
                  <button
                    type="button"
                    className="workspace-subagent-row"
                    disabled={!sessionIds?.has(subagent.id) || onOpenSession === undefined}
                    aria-label={`${subagentTitle(subagent.title, subagent.id)}, ${subagent.status}${attentionById?.has(subagent.id) ? `, ${attentionById.get(subagent.id)}` : ""}, ${sessionIds?.has(subagent.id) && onOpenSession !== undefined ? "Open in tab" : "Session unavailable"}`}
                    aria-describedby={
                      sentence !== undefined ? `${sentencePrefix}${subagent.id}` : undefined
                    }
                    title={!sessionIds?.has(subagent.id) ? "Session unavailable" : undefined}
                    onClick={() => {
                      close();
                      onOpenSession?.(subagent.id);
                    }}
                  >
                    <span
                      className={`workspace-subagent-status-dot ${subagentDotClass(subagent.status)}`}
                      aria-hidden="true"
                    />
                    <span
                      className="workspace-subagent-row-title"
                      title={subagentTitle(subagent.title, subagent.id)}
                    >
                      {subagentTitle(subagent.title, subagent.id)}
                    </span>
                    {attentionById?.has(subagent.id) ? (
                      <span className="workspace-subagent-attention">
                        {attentionById.get(subagent.id)}
                      </span>
                    ) : null}
                    {!sessionIds?.has(subagent.id) ? <span>Unavailable</span> : null}
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
                    <p
                      className="workspace-subagent-row-failure"
                      id={`${sentencePrefix}${subagent.id}`}
                    >
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
