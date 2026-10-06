import { useEffect, useState } from "react";

function clock(elapsedMs: number): string {
  const total = Math.max(0, Math.floor(elapsedMs / 1000));
  const minutes = Math.floor(total / 60);
  return `${String(minutes).padStart(2, "0")}:${String(total % 60).padStart(2, "0")}`;
}

/**
 * The one line a live turn leaves at the end of the transcript: a quiet dot,
 * "Working…", the time since this view saw the turn begin, and how to stop it.
 * It is mounted only while a turn runs, so a finished turn leaves nothing here.
 * The region is polite and its words never change; the clock ticks outside the
 * announcement, so a screen reader hears the line once, not every second.
 */
export function WorkingLine() {
  const [startedAt] = useState(() => Date.now());
  const [now, setNow] = useState(startedAt);
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return (
    <div className="workspace-working-line" role="status">
      <span className="workspace-working-dot dot-pulse" aria-hidden="true" />
      <span>Working…</span>
      <span className="workspace-working-clock" aria-hidden="true">
        {clock(now - startedAt)}
      </span>
      <span className="workspace-working-hint">esc to interrupt</span>
    </div>
  );
}
