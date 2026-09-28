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
}

export function SubagentMenu({ subagents, statusCounts }: SubagentMenuProps) {
  const pillRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const [open, setOpen] = useState(false);

  const close = useCallback(() => {
    // Focus returns only when the menu had it: the rows are inert, so the
    // menu never holds focus itself — the guard keeps a later focusable row
    // from stranding focus on unmount.
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
  const counts = [
    ...(failed > 0 ? [`${failed} failed`] : []),
    ...(working > 0 ? [`${working} working`] : []),
  ].join(", ");
  // The pill is the only place the app names a subagent: a settled run
  // leaves it chevron-only, every part hidden.
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
        onClick={() => setOpen((value) => !value)}
      >
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
              <div className="workspace-subagent-row" key={subagent.id} role="listitem">
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
              </div>
            ))}
          </div>
        </AnchoredPopover>
      ) : null}
    </div>
  );
}
