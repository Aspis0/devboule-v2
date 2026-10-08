import { useEffect, useState } from "react";

/**
 * The wall clock, re-read once a second while `live` is true, and no timer
 * while it is false. The first read is the mount's, so a row that starts after
 * mount shows 0s until the first tick.
 */
export function useTaskClock(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return undefined;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live]);
  return now;
}
