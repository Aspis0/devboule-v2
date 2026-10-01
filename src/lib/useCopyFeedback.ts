import { useCallback, useEffect, useRef, useState } from "react";
import { copyToClipboard } from "./clipboard";

type Outcome = "copied" | "failed";
type CopyState = "idle" | Outcome;
interface Options {
  resetAfterMs: number | null | ((outcome: Outcome) => number | null);
  clearOnCopy?: boolean;
  preserveOnFailure?: boolean;
  clearTimerAfterWrite?: boolean;
}

export function useCopyFeedback({
  resetAfterMs,
  clearOnCopy = false,
  preserveOnFailure = false,
  clearTimerAfterWrite = false,
}: Options) {
  const [feedback, setFeedback] = useState<{
    key: string;
    state: Outcome;
    announcement: string;
  } | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const request = useRef(0);
  const mounted = useRef(true);

  const clearTimer = useCallback(() => {
    if (timer.current !== null) clearTimeout(timer.current);
    timer.current = null;
  }, []);
  const reset = useCallback(() => {
    request.current += 1;
    clearTimer();
    if (mounted.current) setFeedback(null);
  }, [clearTimer]);

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
      request.current += 1;
      clearTimer();
    };
  }, [clearTimer]);

  async function copy(key: string, text: string, subject?: string) {
    if (!mounted.current) return;
    const current = ++request.current;
    if (!clearTimerAfterWrite) clearTimer();
    if (clearOnCopy) setFeedback(null);
    const copied = await copyToClipboard(text);
    if (!mounted.current || current !== request.current) return;
    if (!copied && preserveOnFailure) return;
    if (clearTimerAfterWrite) clearTimer();
    const state = copied ? "copied" : "failed";
    setFeedback({
      key,
      state,
      announcement: subject === undefined ? "" : `${subject} ${copied ? "copied" : "copy failed"}`,
    });
    const delay = typeof resetAfterMs === "function" ? resetAfterMs(state) : resetAfterMs;
    if (delay !== null) {
      timer.current = setTimeout(() => {
        timer.current = null;
        if (mounted.current && (clearTimerAfterWrite || current === request.current))
          setFeedback(null);
      }, delay);
    }
  }

  const stateFor = (key: string): CopyState => (feedback?.key === key ? feedback.state : "idle");
  return {
    copy,
    reset,
    stateFor,
    labelFor: (key: string, label: string) =>
      stateFor(key) === "idle" ? label : stateFor(key) === "copied" ? "Copied" : "Copy failed",
    announcement: feedback?.announcement ?? "",
  };
}
