import { useCallback, useRef, useState } from "react";
import type { ContextUsage, SessionManifest } from "../../types/ipc";
import type { AgentFinished } from "../../lib/agentSession";
import { usePlanRecordedAt, usePlanUsage } from "../../lib/planUsageStore";
import { contextMeterNumbers, formatContextTokens } from "./contextUsageView";
import { ContextPopover } from "./ContextPopover";
import { MeterBar } from "./statusBar/MeterBar";

export interface ContextMeterProps {
  usage: ContextUsage | null;
  manifest: SessionManifest | null;
  /** The session's last finished turn, or null before the first one — the
      popover's cost row reads the figure it carried. */
  lastFinished: AgentFinished | null;
}

/**
 * The status bar's context reading: `ctx 24k/1m` and a thin bar. It shows a
 * ratio only when both sides are known for the session's current model and
 * agree (the reading not above its own window); with one usable side it shows
 * that number alone; with nothing to say it renders nothing at all. Click
 * opens {@link ContextPopover} above it.
 */
export function ContextMeter({ usage, manifest, lastFinished }: ContextMeterProps) {
  const [open, setOpen] = useState(false);
  // The popover anchors to the button, not to the span: it is centred above
  // this rect, and it renders on the body (see ContextPopover), so the
  // Escape/outside-close lives with the panel that can see both elements.
  const buttonRef = useRef<HTMLButtonElement>(null);
  const numbers = contextMeterNumbers(usage, manifest);
  const plan = usePlanUsage(manifest?.providerId ?? null);
  const planRecordedAt = usePlanRecordedAt(manifest?.providerId ?? null);

  const close = useCallback(() => setOpen(false), []);

  const { used, max, percent } = numbers;
  // A reading always names its used side, so no used count is no reading.
  if (used === null) return null;

  const hasRatio = max !== null && percent !== null;
  // With only one side of the ratio known there is no percent to show — the
  // provider sent one number, so one number is what the row says.
  const label = hasRatio
    ? `${formatContextTokens(used)}/${formatContextTokens(max)}`
    : formatContextTokens(used);
  // The name carries the number it shows, so a screen reader hears the reading.
  const spokenLabel = hasRatio
    ? `${formatContextTokens(used)} of ${formatContextTokens(max)}`
    : formatContextTokens(used);

  return (
    <span className="workspace-context-meter">
      <button
        ref={buttonRef}
        type="button"
        className="workspace-context-meter-button"
        aria-label={`Context usage ${spokenLabel}`}
        aria-expanded={open}
        title={hasRatio ? `${percent}% of the context window` : undefined}
        onClick={() => setOpen((previous) => !previous)}
      >
        <span className="workspace-context-meter-text">{`ctx ${label}`}</span>
        <MeterBar percent={hasRatio ? percent : null} />
      </button>
      <ContextPopover
        open={open}
        anchorRef={buttonRef}
        onClose={close}
        numbers={numbers}
        live={usage?.live ?? false}
        plan={plan}
        planRecordedAt={planRecordedAt}
        lastFinished={lastFinished}
        providerId={manifest?.providerId}
      />
    </span>
  );
}
