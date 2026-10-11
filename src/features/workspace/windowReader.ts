import { withinMs } from "../../lib/boundedWait";
import type { WindowState } from "./attentionNotice";

/**
 * What a window-state read can say to a caller that cannot wait on it:
 * - `answered`: a read finished in time; `state` is its answer.
 * - `stale`: the window has not answered in time, or it is still hung from an
 *   earlier read; `state` is the last answer it did give.
 * - `failed`: the window refused to say; the caller must treat it as not
 *   visible (the window might be in the tray).
 * - `unknown`: it has never answered and is not answering now.
 */
export type WindowAnswer =
  | { reason: "answered" | "stale"; state: WindowState }
  | { reason: "failed" | "unknown"; state: null };

export interface WindowReader {
  read(timeoutMs: number, label: string): Promise<WindowAnswer>;
  /** Settles when the read in flight, if any, settles. Resolves at once when none is out. */
  whenSettled(): Promise<void>;
}

interface ReaderEntry {
  /** The last answer the window gave. Never a guess. */
  lastKnown: WindowState | null;
  /** The one read in flight; null while none is out. Resolves to null on refusal. */
  pending: Promise<WindowState | null> | null;
  /** A read ran past its bound and has not settled: no new read is started. */
  hung: boolean;
}

/**
 * One reader per window-state source. Concurrent callers share the read in
 * flight, so each gets a fresh answer when there is one. A read that runs past
 * its bound marks the source hung and keeps its slot until it settles: while
 * hung, no second read starts, so a permanently hung source costs one promise,
 * not one per poll, and callers get the last-known answer instead.
 */
const readers = new WeakMap<() => Promise<WindowState>, WindowReader>();

export function windowReaderFor(source: () => Promise<WindowState>): WindowReader {
  const existing = readers.get(source);
  if (existing !== undefined) return existing;
  const entry: ReaderEntry = { lastKnown: null, pending: null, hung: false };
  const stale = (): WindowAnswer =>
    entry.lastKnown === null
      ? { reason: "unknown", state: null }
      : { reason: "stale", state: entry.lastKnown };
  const reader: WindowReader = {
    async read(timeoutMs, label) {
      if (entry.pending === null) {
        const tracked: Promise<WindowState | null> = source()
          .then(
            (state) => {
              entry.lastKnown = state;
              return state;
            },
            () => null,
          )
          .then((state) => {
            if (entry.pending === tracked) entry.pending = null;
            entry.hung = false;
            return state;
          });
        entry.pending = tracked;
      }
      if (entry.hung) return stale();
      const inFlight = entry.pending;
      let answered: WindowState | null;
      try {
        answered = await withinMs(label, timeoutMs, inFlight);
      } catch {
        entry.hung = true;
        return stale();
      }
      if (answered === null) return { reason: "failed", state: null };
      return { reason: "answered", state: answered };
    },
    async whenSettled() {
      if (entry.pending === null) return;
      await entry.pending;
    },
  };
  readers.set(source, reader);
  return reader;
}
