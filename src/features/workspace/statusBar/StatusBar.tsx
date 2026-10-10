import { useEffect, useState } from "react";
import type { DaemonStatus, Session } from "../../../types/ipc";
import { usePlanRecordedAt, usePlanUsage } from "../../../lib/planUsageStore";
import type { PlanUsage } from "../../../types/ipc";
import { activeSessionAttention, sessionAttentionLabel } from "../sessionAttention";
import { rosterStateDisplay } from "../sessionStateDisplay";
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
  /** The dot's tone for that attention: a failed turn is red, every other ask is ochre. */
  attentionTone: "attention" | "failed" | null;
  working: boolean;
  /** The chip's own word for a non-live state (Quiet, Recovered, Stopped); null while live. */
  stateWord: string | null;
  /** The provider whose plan usage the bar shows for this agent. */
  providerId?: string | null;
}

function attentionToneOf(session: Session): FocusedAgent["attentionTone"] {
  const reason = activeSessionAttention(session)?.reason;
  if (reason === undefined) return null;
  return reason === "error" ? "failed" : "attention";
}

/** The bar's share of a session: the chip's state word for every state but live, which the bar words itself. */
export function agentStateOf(session: Session): Omit<FocusedAgent, "sessionId" | "title"> {
  const live = session.state.type === "live";
  return {
    attentionWord: sessionAttentionLabel(session),
    attentionTone: attentionToneOf(session),
    working: live && session.activity === "working",
    stateWord: live
      ? null
      : rosterStateDisplay(session.state, session.elapsedMs, session.activity).word,
    providerId: session.provider ?? session.kind,
  };
}

interface StatusBarProps {
  agent: FocusedAgent | null;
  daemon: DaemonStatus;
  /** What the app waits on ("Starting…", "Loading…"); it takes the title's place. */
  progress?: string | null;
}

const TITLE_CHARS = 28;

/** Cut by code point, so a surrogate pair is never split. */
function clipTitle(title: string): string {
  const chars = [...title];
  return chars.length > TITLE_CHARS ? `${chars.slice(0, TITLE_CHARS).join("")}…` : title;
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
  // The line shows one window: the 5-hour one when it is current, else the first
  // current one, named so it is never read as the 5-hour figure. The rest wait in the hover.
  const primary = meter.parts.find((part) => part.label === "5h") ?? meter.parts[0];
  const label =
    primary === undefined
      ? meter.name
      : `${meter.name} ${primary.percent}%${primary.label === "5h" ? "" : ` ${primary.label}`}`;
  // A reading ten minutes to an hour old stays in the bar, dimmed; the hover says how old it is.
  return (
    <span
      className={meter.stale ? "status-bar-provider status-meter--stale" : "status-bar-provider"}
      title={meter.title}
    >
      <span>{label}</span>
      <MeterBar percent={meter.barPercent} />
    </span>
  );
}

/**
 * The app's one status line, under every panel: who is working, the focused
 * agent's provider plan usage, and the daemon. The focused agent's context sits
 * beside its composer instead. It shows only what the app holds — a provider
 * that has sent no usage, or none with a percent in it, simply has no meter.
 */
export function StatusBar({ agent, daemon, progress = null }: StatusBarProps) {
  const nowMs = useMinuteClock();
  const reading = useAgentReading(agent?.sessionId ?? null);
  // A plan step counts only while the agent is on one: idle reads idle.
  const state =
    agent === null
      ? null
      : (agent.attentionWord ??
        agent.stateWord ??
        (agent.working ? (reading?.task ?? "working") : "idle"));
  const stateTone =
    agent === null ? "border" : (agent.attentionTone ?? (agent.working ? "green" : "border"));
  const tooltip = daemonLabel(daemon);
  // Only the focused agent's provider is subscribed: a frame for any other provider
  // leaves this bar as it is.
  const focusedPlan = usePlanUsage(agent?.providerId ?? null) ?? undefined;
  return (
    <div className="workspace-status-bar" role="group" aria-label="Status">
      {/* Always mounted, so the words that arrive in it are announced. */}
      <span className="sr-only" role="status">
        {progress}
      </span>
      {progress !== null ? (
        <span className="status-bar-who">
          <span className="status-bar-state" aria-hidden="true">
            {progress}
          </span>
        </span>
      ) : agent === null ? null : (
        <span className="status-bar-who">
          <span className={`workspace-status-dot workspace-dot-${stateTone}`} />
          <span className="status-bar-state">
            {agent.title.length === 0 ? null : (
              <>
                <span className="status-bar-title">{clipTitle(agent.title)}</span>
                <span className="status-bar-sep"> — </span>
              </>
            )}
            {state}
          </span>
        </span>
      )}
      {focusedPlan === undefined ? null : (
        <ProviderUsage key={focusedPlan.providerId} plan={focusedPlan} nowMs={nowMs} />
      )}
      <span className="status-bar-spacer" />
      <span className="status-bar-daemon" title={tooltip}>
        <span className={`workspace-status-dot workspace-dot-${daemonDotTone(daemon.state)}`} />
        <span className="sr-only">{tooltip}</span>
      </span>
    </div>
  );
}
