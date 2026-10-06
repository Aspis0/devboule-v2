import { useEffect, useState } from "react";
import type { DaemonStatus } from "../../../types/ipc";
import { usePlanRecordedAt, useAllPlanUsage } from "../../../lib/planUsageStore";
import type { PlanUsage } from "../../../types/ipc";
import { ContextMeter } from "../ContextMeter";
import { daemonDotTone, daemonLabel } from "../sidebar/SidebarFooter";
import { useAgentReading } from "./agentReadingStore";
import { MeterBar } from "./MeterBar";
import { providerMeter } from "./planMeters";
import "./StatusBar.css";

/** The agent in the focused pane, as the bar names it. */
export interface FocusedAgent {
  sessionId: string;
  title: string;
  /** The attention the roster raised ("Needs your approval"), when there is one. */
  attentionWord: string | null;
  working: boolean;
}

interface StatusBarProps {
  agent: FocusedAgent | null;
  daemon: DaemonStatus;
}

/** The time, moved on each minute: the tooltips count resets and ages in minutes. */
function useMinuteClock(): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 60_000);
    return () => clearInterval(timer);
  }, []);
  return now;
}

function ProviderUsage({ plan, nowMs }: { plan: PlanUsage; nowMs: number }) {
  const recordedAt = usePlanRecordedAt(plan.providerId);
  const meter = providerMeter(plan, recordedAt, nowMs);
  if (meter === null) return null;
  return (
    <span className="status-bar-provider" title={meter.title}>
      <span>
        {`${meter.name} ${meter.parts.map((part) => `${part.percent}% ${part.label}`).join(" · ")}`}
      </span>
      <MeterBar percent={meter.barPercent} />
    </span>
  );
}

function FocusedAgentContext({ sessionId }: { sessionId: string }) {
  const reading = useAgentReading(sessionId);
  if (reading === null) return null;
  return (
    <ContextMeter
      usage={reading.usage}
      manifest={reading.manifest}
      lastFinished={reading.lastFinished}
    />
  );
}

/**
 * The app's one status line, under every panel: who is working, each
 * provider's plan usage, the focused agent's context, and the daemon. It shows
 * only what the app holds — a provider that has sent no usage, or none with a
 * percent in it, simply has no meter.
 */
export function StatusBar({ agent, daemon }: StatusBarProps) {
  const frames = useAllPlanUsage();
  const nowMs = useMinuteClock();
  const reading = useAgentReading(agent?.sessionId ?? null);
  // A plan step counts only while the agent is on one: idle reads idle.
  const state =
    agent === null
      ? null
      : (agent.attentionWord ?? (agent.working ? (reading?.task ?? "working") : "idle"));
  const tooltip = daemonLabel(daemon);
  return (
    <div className="workspace-status-bar" role="group" aria-label="Status">
      {agent === null ? null : (
        <span className="status-bar-who">
          <span className="status-bar-state">
            {agent.title}
            <span className="status-bar-sep"> — </span>
            {state}
          </span>
        </span>
      )}
      {frames.map((plan) => (
        <ProviderUsage key={plan.providerId} plan={plan} nowMs={nowMs} />
      ))}
      <span className="status-bar-spacer" />
      {agent === null ? null : <FocusedAgentContext sessionId={agent.sessionId} />}
      <span className="status-bar-daemon" title={tooltip}>
        <span className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`} />
        <span className="sr-only">{tooltip}</span>
      </span>
    </div>
  );
}
