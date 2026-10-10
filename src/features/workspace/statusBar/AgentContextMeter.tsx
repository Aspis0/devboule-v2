import { ContextMeter } from "../ContextMeter";
import { useAgentReading } from "./agentReadingStore";

/**
 * The focused agent's context, beside its composer. It reads the reading store
 * on its own, so the surface that holds it never re-renders for a new reading.
 */
export function AgentContextMeter({ sessionId }: { sessionId: string }) {
  const reading = useAgentReading(sessionId);
  if (reading === null) return null;
  return (
    <span className="workspace-composer-context">
      <ContextMeter
        usage={reading.usage}
        manifest={reading.manifest}
        lastFinished={reading.lastFinished}
      />
    </span>
  );
}
