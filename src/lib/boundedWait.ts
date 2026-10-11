/**
 * A promise the caller will not wait on forever. The window and the OS answer
 * through IPC; when one of those calls never settles, a toast or a presence
 * report that awaits it would stall for good. The bound turns that silence
 * into a rejection the caller already knows how to treat.
 */
export class BoundedWaitTimeout extends Error {
  constructor(label: string, ms: number) {
    super(`${label} did not answer within ${ms} ms`);
    this.name = "BoundedWaitTimeout";
  }
}

export function withinMs<T>(label: string, ms: number, pending: Promise<T>): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expired = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => reject(new BoundedWaitTimeout(label, ms)), ms);
  });
  return Promise.race([pending, expired]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

/**
 * The attention path's own clock: `devboule: debug:` lines, one per stage, with
 * the wall-clock millisecond the stage ran at. Off unless the webview's
 * `localStorage` carries `devboule.debug.attention = "1"`, so a live run is
 * measured with one switch and a normal run stays silent.
 */
export function traceAttention(stage: string, detail: Record<string, string | number>): void {
  if (typeof localStorage === "undefined") return;
  if (localStorage.getItem("devboule.debug.attention") !== "1") return;
  const fields = Object.entries(detail)
    .map(([key, value]) => `${key}=${value}`)
    .join(" ");
  console.debug(`devboule: debug: attention ${stage} at=${Date.now()} ${fields}`);
}
