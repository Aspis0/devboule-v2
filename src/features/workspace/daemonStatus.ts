import { errorSentence } from "../../lib/errorSentence";
import type { DaemonStatus } from "../../types/ipc";

/** Green while the daemon answers, hollow while connecting, red otherwise. */
export function daemonDotTone(state: DaemonStatus["state"]): string {
  if (state === "connected") return "green";
  if (state === "connecting") return "border";
  return "terracotta";
}

/** The status bar's tooltip: the whole daemon sentence, not the quiet label.
 * The reported message goes through the mapper, so a known daemon failure
 * reads as advice instead of the supervisor's own diagnosis. */
export function daemonLabel(status: DaemonStatus): string {
  const reason = status.message === null ? null : errorSentence(status.message).sentence;
  if (status.state === "connected") {
    const pid = status.pid !== null ? `pid ${status.pid}` : "connected";
    return reason === null ? `daemon · ${pid}` : `daemon · ${pid} · ${reason}`;
  }
  if (status.state === "connecting") return "daemon · connecting";
  if (status.state === "unresponsive") {
    return reason === null ? "daemon · not answering" : `daemon · ${reason}`;
  }
  return reason === null ? "daemon · disconnected" : `daemon · ${reason}`;
}
