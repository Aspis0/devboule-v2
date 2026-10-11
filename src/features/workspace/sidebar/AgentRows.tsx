import { StripKindMark } from "../strip/StripKindMark";
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

/** The states a row names in its tail, and the tone the name takes: a badge alone cannot say them. */
const TAIL_TONE: Partial<Record<RailAgentState, string>> = {
  waiting: "attention",
  failed: "danger",
  stopped: "muted",
};

/** The states a row marks with a badge; a settled agent is marked by its mark alone. */
const BADGED: ReadonlySet<RailAgentState> = new Set(["running", "waiting", "failed"]);

function AgentRow({
  agent,
  active,
  onOpen,
}: {
  agent: RailAgent;
  active: boolean;
  onOpen: (sessionId: string) => void;
}) {
  const word = STATE_WORD[agent.state];
  const tone = TAIL_TONE[agent.state];
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
  return (
    <button
      type="button"
      className={classes}
      aria-current={active ? "true" : undefined}
      aria-label={label}
      title={tooltip}
      onClick={() => onOpen(agent.id)}
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
    </button>
  );
}

export interface AgentRowsProps {
  agents: RailAgents;
  /** The agent whose tab is in front: its row takes the hover fill and nothing more. */
  activeId: string | null;
  onOpenAgent: (sessionId: string) => void;
  onOpenHistory: () => void;
}

/** The agents of the selected workspace, nested under its row, capped and ending in `+N more in History`. */
export function AgentRows({ agents, activeId, onOpenAgent, onOpenHistory }: AgentRowsProps) {
  if (agents.rows.length === 0) return null;
  return (
    <div className="agent-rows">
      {agents.rows.map((agent) => (
        <AgentRow
          key={agent.id}
          agent={agent}
          active={agent.id === activeId}
          onOpen={onOpenAgent}
        />
      ))}
      {agents.hidden === 0 ? null : (
        <button type="button" className="agent-row-more" onClick={onOpenHistory}>
          +{agents.hidden} more in History
        </button>
      )}
    </div>
  );
}
