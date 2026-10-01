// The subagent pill in the pane header and the menu it opens. The menu
// renders through the shared anchored portal because the pill sits at the
// top of the centre panel, where the panel's overflow would clip whatever
// opens from it. Which side the menu opens on is the portal's placement
// call: above when the content fits above, below over the transcript when
// it does not.
import { useCallback, useEffect, useId, useRef, useState } from "react";
import { AnchoredPopover } from "./popoverPlace";
import { useMenuOpen } from "../../lib/menuOpen";
import "./SubagentMenu.css";
import type {
  AgentSubagent,
  AgentSubagentStatus,
  AgentSubagentStatusCounts,
} from "../../lib/agentSession";

function shortSubagentId(id: string): string {
  return id.length > 16 ? `${id.slice(0, 12)}…` : id;
}

function subagentTitle(title: string | null, id: string): string {
  return title?.trim() ? title : shortSubagentId(id);
}

function subagentDotClass(status: AgentSubagentStatus): string {
  return `workspace-subagent-status-${status}`;
}

export interface SubagentMenuProps {
  subagents: AgentSubagent[];
  statusCounts: AgentSubagentStatusCounts;
  onOpenSession?: (sessionId: string) => void;
  sessionIds?: ReadonlySet<string>;
  attentionById?: ReadonlyMap<string, string>;
  onRefreshSessions?: () => Promise<void>;
}

export function SubagentMenu({
  subagents,
  statusCounts,
  onOpenSession,
  sessionIds,
  attentionById,
  onRefreshSessions,
}: SubagentMenuProps) {
  const pillRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [open, setOpen] = useState(false);

  const close = useCallback(() => {
    if (menuRef.current?.contains(document.activeElement)) {
      pillRef.current?.focus({ preventScroll: true });
    }
    setOpen(false);
  }, []);

  useMenuOpen(open, close);

  useEffect(() => {
    if (!open) return;
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
  }, [open, close]);

  if (subagents.length === 0) return null;

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
          <div className="workspace-subagent-list-head">Subagents</div>
          <div role="list">
            {subagents.map((subagent) => (
              <div key={subagent.id} role="listitem">
                <button
                  type="button"
                  className="workspace-subagent-row"
                  disabled={!sessionIds?.has(subagent.id) || onOpenSession === undefined}
                  aria-label={`${subagentTitle(subagent.title, subagent.id)}, ${subagent.status}${attentionById?.has(subagent.id) ? `, ${attentionById.get(subagent.id)}` : ""}, ${sessionIds?.has(subagent.id) && onOpenSession !== undefined ? "Open in tab" : "Session unavailable"}`}
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
              </div>
            ))}
          </div>
        </AnchoredPopover>
      ) : null}
    </div>
  );
}
