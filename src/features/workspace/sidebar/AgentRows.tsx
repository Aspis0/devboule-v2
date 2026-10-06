import { memo } from "react";
import { StripKindMark } from "../strip/StripKindMark";
import { useAgentReading } from "../statusBar/agentReadingStore";
import type { AgentRowView } from "./agentRowViews";

interface AgentRowProps {
  agent: AgentRowView;
  active: boolean;
  onOpen: (sessionId: string) => void;
}

/**
 * One agent under its workspace: its mark, its name, and what it is doing with
 * how long it has been quiet. The step it is on is known only to the surface
 * that is open for it, so a working agent says its step when there is one and
 * `working` when there is not.
 */
const AgentRow = memo(function AgentRow({ agent, active, onOpen }: AgentRowProps) {
  const task = useAgentReading(agent.working ? agent.id : null)?.task ?? null;
  const doing = agent.working ? (task ?? agent.word) : agent.word;
  const sub = agent.working || agent.age === null ? doing : `${doing} · ${agent.age}`;
  return (
    <button
      type="button"
      className={`workspace-agent-row${active ? " workspace-agent-row-active" : ""}`}
      aria-current={active ? "true" : undefined}
      aria-label={`${agent.title}, ${sub}`}
      onClick={() => onOpen(agent.id)}
    >
      <StripKindMark kind={agent.kind} />
      <span className="workspace-agent-name">{agent.title}</span>
      <span className={`workspace-agent-sub${agent.attention ? " sidebar-row-waiting" : ""}`}>
        {sub}
      </span>
    </button>
  );
});

interface AgentRowsProps {
  agents: readonly AgentRowView[];
  /** The agent whose tab is in front, if one is. */
  activeSessionId: string | null;
  onOpen: (sessionId: string) => void;
}

/** The agents of the selected workspace, nested under its row. */
export function AgentRows({ agents, activeSessionId, onOpen }: AgentRowsProps) {
  if (agents.length === 0) return null;
  return (
    <div className="workspace-agent-rows">
      {agents.map((agent) => (
        <AgentRow
          key={agent.id}
          agent={agent}
          active={agent.id === activeSessionId}
          onOpen={onOpen}
        />
      ))}
    </div>
  );
}
