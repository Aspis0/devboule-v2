import { useRef, useState, type KeyboardEvent, type MouseEvent } from "react";
import { StripKindMark } from "../strip/StripKindMark";
import type { ArchiveTarget } from "../subagentArchive";
import { AgentRowMenu } from "./AgentRowMenu";
import { focusWorkspaceRowAbove, treeKeyTarget } from "./agentTreeKeys";
import type { RailAgent, RailAgentState, RailAgents } from "./railAgents";
import "./agentRows.css";

const STATE_WORD: Record<RailAgentState, string> = {
  running: "working",
  waiting: "needs you",
  finished: "finished",
  failed: "failed",
  stopped: "stopped",
  idle: "idle",
};

/** The states that carry a badge on the mark; a settled agent is marked by its mark alone. */
const BADGED: ReadonlySet<RailAgentState> = new Set(["running", "waiting", "failed"]);

/** The states whose word shows in the tail, with the tail's colour. */
const TAIL_TONE: Partial<Record<RailAgentState, string>> = {
  waiting: "attention",
  failed: "danger",
  stopped: "muted",
};

export interface AgentRowsProps {
  agents: RailAgents;
  /** The agent whose tab is in front: its row takes the hover fill and nothing more. */
  activeId: string | null;
  onOpenAgent: (sessionId: string) => void;
  onOpenHistory: () => void;
  /** Archives a child as the row showed it; answers with the refusal sentence, or null. */
  onArchiveAgent?: (target: ArchiveTarget) => Promise<string | null>;
}

interface RowProps {
  agent: RailAgent;
  active: boolean;
  tabStop: boolean;
  onFocusRow: () => void;
  onKeyDown: (event: KeyboardEvent<HTMLDivElement>) => void;
  onMenu: ((event: MouseEvent<HTMLDivElement>) => void) | null;
  onOpen: () => void;
  rowRef: (node: HTMLDivElement | null) => void;
}

function AgentRow({
  agent,
  active,
  tabStop,
  onFocusRow,
  onKeyDown,
  onMenu,
  onOpen,
  rowRef,
}: RowProps) {
  const word = STATE_WORD[agent.state];
  const label = [
    agent.title,
    word,
    agent.provider,
    agent.parentTitle && `child of ${agent.parentTitle}`,
  ]
    .filter(Boolean)
    .join(", ");
  const tooltip = [agent.title, agent.provider, word].filter(Boolean).join(" · ");
  const classes = [
    "agent-row",
    agent.child ? "agent-row-child" : "",
    active ? "agent-row-active" : "",
    agent.state === "finished" || agent.state === "stopped" ? "agent-row-settled" : "",
  ]
    .filter(Boolean)
    .join(" ");
  const tone = TAIL_TONE[agent.state];
  return (
    <div
      role="treeitem"
      ref={rowRef}
      className={classes}
      tabIndex={tabStop ? 0 : -1}
      aria-level={agent.child ? 2 : 1}
      aria-expanded={agent.hasChildren ? true : undefined}
      aria-current={active ? "true" : undefined}
      aria-label={label}
      title={tooltip}
      onClick={onOpen}
      onFocus={onFocusRow}
      onKeyDown={onKeyDown}
      onContextMenu={onMenu ?? undefined}
    >
      <span className="agent-row-lane">
        <StripKindMark kind={agent.kind} />
        {BADGED.has(agent.state) ? (
          <span className={`agent-row-badge agent-row-badge-${agent.state}`} aria-hidden="true" />
        ) : null}
      </span>
      <span className="agent-row-name">{agent.title}</span>
      {tone === undefined ? null : (
        <span className={`agent-row-tail agent-row-tail-${tone}`}>{word}</span>
      )}
    </div>
  );
}

interface OpenMenu {
  target: ArchiveTarget;
  title: string;
  anchor: HTMLElement;
}

/**
 * The agents of the selected workspace, nested under its row, as one tree.
 * Arrows, Home and End move focus through it; Up or Left from its top leaves it
 * for the workspace row; Enter opens the agent; an archivable agent's menu offers
 * Archive. Past the cap, a `+N more in History` link.
 */
export function AgentRows({
  agents,
  activeId,
  onOpenAgent,
  onOpenHistory,
  onArchiveAgent,
}: AgentRowsProps) {
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [menu, setMenu] = useState<OpenMenu | null>(null);
  const itemRefs = useRef(new Map<string, HTMLDivElement>());
  const rootRef = useRef<HTMLDivElement>(null);
  const rows = agents.rows;
  if (rows.length === 0) return null;
  const tabStopId =
    focusedId !== null && rows.some((row) => row.id === focusedId)
      ? focusedId
      : (rows.find((row) => row.id === activeId)?.id ?? rows[0]?.id);

  const focusRow = (index: number) => {
    const row = rows[index];
    if (row === undefined) return;
    setFocusedId(row.id);
    itemRefs.current.get(row.id)?.focus();
  };

  // The menu takes the generation the row shows now: the archive checks it when
  // the person confirms, so a newer generation in between is refused.
  const openMenu = (row: RailAgent, anchor: HTMLElement) => {
    if (!row.archivable || onArchiveAgent === undefined || row.generation === null) return;
    setMenu({ target: { id: row.id, generation: row.generation }, title: row.title, anchor });
  };

  return (
    <div className="agent-rows" ref={rootRef}>
      <div role="tree" aria-label="Agents">
        {rows.map((agent, index) => (
          <AgentRow
            key={agent.id}
            agent={agent}
            active={agent.id === activeId}
            tabStop={agent.id === tabStopId}
            onFocusRow={() => setFocusedId(agent.id)}
            onOpen={() => onOpenAgent(agent.id)}
            onKeyDown={(event) => {
              if (event.key === "Enter" || event.key === " ") {
                event.preventDefault();
                onOpenAgent(agent.id);
                return;
              }
              if (event.key === "ContextMenu" || (event.shiftKey && event.key === "F10")) {
                event.preventDefault();
                openMenu(agent, event.currentTarget);
                return;
              }
              const target = treeKeyTarget(event.key, index, rows);
              if (target !== null) {
                event.preventDefault();
                focusRow(target);
                return;
              }
              if (
                (event.key === "ArrowUp" || event.key === "ArrowLeft") &&
                rootRef.current !== null &&
                focusWorkspaceRowAbove(rootRef.current)
              ) {
                event.preventDefault();
              }
            }}
            onMenu={
              agent.archivable && onArchiveAgent !== undefined
                ? (event) => {
                    event.preventDefault();
                    openMenu(agent, event.currentTarget);
                  }
                : null
            }
            rowRef={(node) => {
              if (node === null) itemRefs.current.delete(agent.id);
              else itemRefs.current.set(agent.id, node);
            }}
          />
        ))}
      </div>
      {agents.hidden === 0 ? null : (
        <button type="button" className="agent-row-more" onClick={onOpenHistory}>
          +{agents.hidden} more in History
        </button>
      )}
      {menu === null || onArchiveAgent === undefined ? null : (
        <AgentRowMenu
          target={menu.target}
          title={menu.title}
          anchor={{ current: menu.anchor }}
          onArchive={onArchiveAgent}
          onDismiss={() => {
            itemRefs.current.get(menu.target.id)?.focus();
            setMenu(null);
          }}
        />
      )}
    </div>
  );
}
