import { useCallback, useEffect, useRef, useState } from "react";
import type { ContextUsage, SessionManifest } from "../../types/ipc";
import { usePlanUsage } from "../../lib/planUsageStore";
import { contextMeterNumbers, formatContextTokens } from "./contextUsageView";
import { ContextPopover } from "./ContextPopover";

const RING_RADIUS = 6;
const RING_CIRCUMFERENCE = 2 * Math.PI * RING_RADIUS;

export interface ContextMeterProps {
  usage: ContextUsage | null;
  manifest: SessionManifest | null;
  /** A turn is running: with no reading yet, the track-only ring reserves
      the spot instead of leaving the row to jump later. */
  running: boolean;
}

/**
 * The composer's context ring. It shows a percentage only when both sides of
 * the ratio are known for the session's current model; with just one side it
 * shows that number without a percent; while a turn runs without any reading
 * it shows the track alone; and with nothing to say and nothing running it
 * renders nothing at all. Click opens {@link ContextPopover} above it.
 */
export function ContextMeter({ usage, manifest, running }: ContextMeterProps) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLSpanElement>(null);
  const numbers = contextMeterNumbers(usage, manifest);
  const plan = usePlanUsage(manifest?.providerId ?? null);

  const close = useCallback(() => setOpen(false), []);
  useEffect(() => {
    if (!open) return;
    const onKeyDown = (event: globalThis.KeyboardEvent): void => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close();
    };
    const onPointerDown = (event: PointerEvent): void => {
      const target = event.target;
      if (target instanceof Node && !rootRef.current?.contains(target)) close();
    };
    document.addEventListener("keydown", onKeyDown);
    document.addEventListener("pointerdown", onPointerDown);
    return () => {
      document.removeEventListener("keydown", onKeyDown);
      document.removeEventListener("pointerdown", onPointerDown);
    };
  }, [close, open]);

  const { used, max, percent } = numbers;
  if (used === null && max === null && !running) return null;

  const hasRatio = used !== null && max !== null && percent !== null;
  // With only one side of the ratio known there is no percent to show — the
  // provider sent one number, so one number is what the row says.
  const label = hasRatio
    ? `${percent}% · ${formatContextTokens(used)} / ${formatContextTokens(max)}`
    : used !== null
      ? formatContextTokens(used)
      : null;
  const arcPercent = hasRatio ? Math.max(0, Math.min(100, percent)) : null;

  return (
    <span className="workspace-context-meter" ref={rootRef}>
      <button
        type="button"
        className="workspace-context-meter-button"
        aria-label="Context usage"
        aria-expanded={open}
        onClick={() => setOpen((previous) => !previous)}
      >
        <svg
          width={14}
          height={14}
          viewBox="0 0 14 14"
          className="workspace-context-meter-ring"
          aria-hidden="true"
        >
          <circle
            cx={7}
            cy={7}
            r={RING_RADIUS}
            fill="none"
            stroke="var(--line-strong)"
            strokeWidth={2}
          />
          {arcPercent !== null ? (
            <circle
              cx={7}
              cy={7}
              r={RING_RADIUS}
              fill="none"
              stroke="var(--accent)"
              strokeWidth={2}
              strokeLinecap="round"
              strokeDasharray={RING_CIRCUMFERENCE}
              strokeDashoffset={RING_CIRCUMFERENCE * (1 - arcPercent / 100)}
              transform="rotate(-90 7 7)"
            />
          ) : null}
        </svg>
        {label !== null ? <span className="workspace-context-meter-text">{label}</span> : null}
      </button>
      {open ? <ContextPopover numbers={numbers} live={usage?.live ?? false} plan={plan} /> : null}
    </span>
  );
}
