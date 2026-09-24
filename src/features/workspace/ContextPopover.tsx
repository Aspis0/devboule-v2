import { useEffect, useState, type ReactNode } from "react";
import type { PlanUsage } from "../../types/ipc";
import {
  formatContextTokens,
  planWindowLabel,
  planWindowMeta,
  type ContextMeterNumbers,
} from "./contextUsageView";

const POPOVER_RING_RADIUS = 12;
const POPOVER_RING_CIRCUMFERENCE = 2 * Math.PI * POPOVER_RING_RADIUS;

export interface ContextPopoverProps {
  numbers: ContextMeterNumbers;
  /** Whether the reading is a mid-turn push (`true`) or the end of the last
      turn (`false`, labelled as such). */
  live: boolean;
  /** The provider's latest plan frame, or null when it reports none. */
  plan: PlanUsage | null;
}

function readingBody(numbers: ContextMeterNumbers, live: boolean): ReactNode {
  const { used, max, percent } = numbers;
  const stale = live ? null : <div className="workspace-context-note">as of the last turn</div>;
  if (used !== null && max !== null && percent !== null) {
    const offset = POPOVER_RING_CIRCUMFERENCE * (1 - Math.max(0, Math.min(100, percent)) / 100);
    return (
      <>
        <svg
          width={26}
          height={26}
          viewBox="0 0 26 26"
          className="workspace-context-ring"
          aria-hidden="true"
        >
          <circle
            cx={13}
            cy={13}
            r={POPOVER_RING_RADIUS}
            fill="none"
            stroke="var(--line-strong)"
            strokeWidth={2}
          />
          <circle
            cx={13}
            cy={13}
            r={POPOVER_RING_RADIUS}
            fill="none"
            stroke="var(--accent)"
            strokeWidth={2}
            strokeLinecap="round"
            strokeDasharray={POPOVER_RING_CIRCUMFERENCE}
            strokeDashoffset={offset}
            transform="rotate(-90 13 13)"
          />
        </svg>
        <div className="workspace-context-reading">
          <div className="workspace-context-percent">{percent}% used</div>
          <div className="workspace-context-tokens">
            {formatContextTokens(used)} / {formatContextTokens(max)} tokens
          </div>
          {stale}
        </div>
      </>
    );
  }
  if (used !== null) {
    return (
      <div className="workspace-context-reading">
        <div className="workspace-context-tokens">{formatContextTokens(used)} tokens used</div>
        {stale}
      </div>
    );
  }
  return (
    <div className="workspace-context-reading">
      <div className="workspace-context-note">No context reading yet.</div>
    </div>
  );
}

function planBody(plan: PlanUsage | null, nowMs: number): ReactNode {
  if (plan === null) {
    return <div className="workspace-context-note">This provider does not report plan usage.</div>;
  }
  return (
    <>
      {plan.planLabel !== undefined ? (
        <span className="workspace-context-plan-chip">{plan.planLabel}</span>
      ) : null}
      {plan.windows.map((window, index) => {
        const meta = planWindowMeta(window, nowMs);
        return (
          // The pair (position, duration) — duration alone is not a key: the
          // frame is free to carry two windows of one length.
          <div className="workspace-context-window" key={`${index}-${window.durationMins}`}>
            <div className="workspace-context-window-row">
              <span className="workspace-context-window-label">
                {planWindowLabel(window.durationMins)}
              </span>
              {meta !== null ? <span className="workspace-context-window-meta">{meta}</span> : null}
            </div>
            <div className="workspace-context-window-bar">
              {window.usedPercent !== undefined ? (
                <div
                  className="workspace-context-window-fill"
                  style={{ width: `${Math.max(0, Math.min(100, window.usedPercent))}%` }}
                />
              ) : null}
            </div>
          </div>
        );
      })}
      {plan.credits !== undefined && plan.credits.unlimited ? (
        <div className="workspace-context-credits">Credits: unlimited</div>
      ) : plan.credits !== undefined && plan.credits.balance !== undefined ? (
        <div className="workspace-context-credits">Credits: {plan.credits.balance}</div>
      ) : null}
    </>
  );
}

/**
 * The context popover:300 px of plain facts above the ring — the reading the
 * meter draws, then the provider's own plan windows, or one sentence saying
 * the provider sends none. Every number here is one the provider sent.
 */
export function ContextPopover({ numbers, live, plan }: ContextPopoverProps) {
  // The countdown must keep counting while the popover sits open; a render
  // that never happens would freeze "resets in" at whatever it said on open.
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const timer = globalThis.setInterval(() => setNowMs(Date.now()), 30_000);
    return () => globalThis.clearInterval(timer);
  }, []);
  return (
    <div className="workspace-context-popover" role="dialog" aria-label="Context usage">
      <div className="workspace-context-popover-head">{readingBody(numbers, live)}</div>
      <div className="workspace-context-popover-plan">{planBody(plan, nowMs)}</div>
    </div>
  );
}
