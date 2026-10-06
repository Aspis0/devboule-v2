import { memo, useState } from "react";
import type { SessionKind } from "../../../types/ipc";
import { StripKindMark } from "../strip/StripKindMark";
import { useAgentReading } from "../statusBar/agentReadingStore";
import type { AgentRowView } from "./agentRowViews";

/** Agents listed before the rest wait behind `+N more`. */
const AGENTS_SHOWN = 8;

interface AgentRowProps {
  id: string;
  kind: SessionKind;
  title: string;
  word: string;
  attention: boolean;
  working: boolean;
  quiet: boolean;
  age: string | null;
  active: boolean;
  onOpen: (sessionId: string) => void;
}

/**
 * One agent under its workspace: its mark, its name, and what it is doing with
 * how long it has been quiet. The step it is on is the one its open surface
 * published, and the controller pauses a step when its turn ends, so a new
 * turn never shows the last one's; an agent whose surface is closed says
 * `working`. A quiet agent says how long, working or not.
 * Primitive props, so a roster push that changes nothing here re-renders nothing.
 */
const AgentRow = memo(function AgentRow({
  id,
  kind,
  title,
  word,
  attention,
  working,
  quiet,
  age,
  active,
  onOpen,
}: AgentRowProps) {
  const task = useAgentReading(working && !quiet ? id : null)?.task ?? null;
  const doing = working && !quiet ? (task ?? word) : word;
  const sub = (working && !quiet) || age === null ? doing : `${doing} · ${age}`;
  return (
    <button
      type="button"
      className={`workspace-agent-row${active ? " workspace-agent-row-active" : ""}`}
      aria-current={active ? "true" : undefined}
      aria-label={`${title}, ${sub}`}
      onClick={() => onOpen(id)}
    >
      <StripKindMark kind={kind} />
      <span className="workspace-agent-name">{title}</span>
      <span className={`workspace-agent-sub${attention ? " sidebar-row-waiting" : ""}`}>{sub}</span>
    </button>
  );
});

interface AgentRowsProps {
  agents: readonly AgentRowView[];
  /** The agent whose tab is in front, if one is. */
  activeSessionId: string | null;
  onOpen: (sessionId: string) => void;
}

/**
 * The agents of the selected workspace, nested under its row. Past the first
 * few they wait behind a `+N more` that opens them in place; the agent in front
 * and any that asks for the person stay listed whatever the cap.
 */
export function AgentRows({ agents, activeSessionId, onOpen }: AgentRowsProps) {
  const [expanded, setExpanded] = useState(false);
  if (agents.length === 0) return null;
  const listed = expanded
    ? agents
    : agents.filter(
        (agent, index) => index < AGENTS_SHOWN || agent.id === activeSessionId || agent.attention,
      );
  const hidden = agents.length - listed.length;
  return (
    <div className="workspace-agent-rows">
      {listed.map((agent) => (
        <AgentRow
          key={agent.id}
          id={agent.id}
          kind={agent.kind}
          title={agent.title}
          word={agent.word}
          attention={agent.attention}
          working={agent.working}
          quiet={agent.quiet}
          age={agent.age}
          active={agent.id === activeSessionId}
          onOpen={onOpen}
        />
      ))}
      {hidden > 0 || (expanded && agents.length > AGENTS_SHOWN) ? (
        <button
          type="button"
          className="workspace-agent-more"
          aria-expanded={expanded}
          onClick={() => setExpanded((open) => !open)}
        >
          {expanded ? "Show fewer" : `+${hidden} more`}
        </button>
      ) : null}
    </div>
  );
}
