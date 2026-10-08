import { useEffect, useState } from "react";

/**
 * The wall clock, re-read once a second while `live` is true, and no timer
 * while it is false. Going live re-reads the clock in the same render, so a row
 * that starts while the clock was idle is measured from now and not from the
 * last time anything ran.
 */
export function useTaskClock(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  const [wasLive, setWasLive] = useState(live);
  if (live !== wasLive) {
    setWasLive(live);
    if (live) setNow(Date.now());
  }
  useEffect(() => {
    if (!live) return undefined;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live]);
  return now;
}
