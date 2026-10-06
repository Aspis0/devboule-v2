import { useEffect, useState } from "react";

function clock(elapsedMs: number): string {
  const total = Math.max(0, Math.floor(elapsedMs / 1000));
  const minutes = Math.floor(total / 60);
  return `${String(minutes).padStart(2, "0")}:${String(total % 60).padStart(2, "0")}`;
}

/** The time since the turn began, ticking; nothing is drawn or scheduled without a start. */
function Elapsed({ startedAtMs }: { startedAtMs: number }) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return (
    <span className="workspace-working-clock" aria-hidden="true">
      {clock(now - startedAtMs)}
    </span>
  );
}

/**
 * The one line a live turn leaves at the end of the transcript: a quiet dot,
 * "Working…", the time since the turn began when that is known, and how to stop
 * it. It is mounted only while a turn runs, so a finished turn leaves nothing
 * here. A turn this view did not start has no known beginning, and says no time
 * rather than the age of the view. The region is polite and its words never
 * change; the clock ticks outside the announcement.
 */
export function WorkingLine({ startedAtMs }: { startedAtMs: number | null }) {
  return (
    <div className="workspace-working-line" role="status">
      <span className="workspace-working-dot" aria-hidden="true" />
      <span>Working…</span>
      {startedAtMs === null ? null : <Elapsed startedAtMs={startedAtMs} />}
      <span className="workspace-working-hint">esc to interrupt</span>
    </div>
  );
}
