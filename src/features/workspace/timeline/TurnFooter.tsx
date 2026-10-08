import type { AgentFinished } from "../../../lib/agentSession";
import { usdCopy } from "../../../lib/format";
import "./TurnFooter.css";

/** Why an abnormal turn stopped. A normal end names no reason: the reply is the answer. */
function stopCopy(finished: AgentFinished): string | null {
  if (!finished.stopReason || isNormalEnd(finished.stopReason)) return null;
  return `stopped: ${finished.stopReason}`;
}

/** Normal ends are the ones the daemon's stop-reason mapping counts as completed. */
function isNormalEnd(stopReason: string): boolean {
  return stopReason === "stop" || stopReason === "end_turn" || stopReason === "completed";
}

/** The token ledger, which is the same count spelled out in five places, then what it cost. */
function detailCopy(finished: AgentFinished, providerId: string | undefined): string | null {
  const usage = finished.usage;
  if (usage === undefined) return null;
  // pi's cost is its own estimate from a static price list, not what the provider
  // billed, so a pi turn shows none.
  const cost = providerId === "pi" || usage.costUsd === undefined ? null : usdCopy(usage.costUsd);
  const details = [
    usage.inputTokens === undefined ? null : `in ${usage.inputTokens.toLocaleString()}`,
    usage.outputTokens === undefined ? null : `out ${usage.outputTokens.toLocaleString()}`,
    usage.cacheReadTokens === undefined ? null : `cached ${usage.cacheReadTokens.toLocaleString()}`,
    usage.cacheWriteTokens === undefined
      ? null
      : `cache-wrote ${usage.cacheWriteTokens.toLocaleString()}`,
    usage.thoughtTokens === undefined ? null : `thought ${usage.thoughtTokens.toLocaleString()}`,
    usage.totalTokens === undefined ? null : `total ${usage.totalTokens.toLocaleString()} tokens`,
    cost,
  ].filter((part): part is string => part !== null);
  return details.length > 0 ? details.join(" · ") : null;
}

/**
 * The finished turn's metadata: a stop line for an abnormal end, and one
 * disclosure carrying the ledger and the cost.
 *
 * The line is what a reader glances at when something went wrong; the
 * disclosure answers "what did that turn spend", and it is a <details> so the
 * keyboard drives it without a key handler and a screen reader gets the native
 * disclosure for free. A normally finished turn takes no line, only the arrow
 * when the daemon sent usage.
 */
export function TurnFooter({
  finished,
  providerId,
}: {
  finished: AgentFinished | null;
  providerId?: string;
}) {
  if (finished === null) return null;
  const stop = stopCopy(finished);
  const ledger = detailCopy(finished, providerId);
  if (stop === null && ledger === null) return null;
  return (
    <div className="turn-footer">
      {stop === null ? null : <div className="turn-footer-line">{stop}</div>}
      {ledger === null ? null : (
        <details className="turn-footer-detail">
          <summary className="turn-footer-detail-trigger" aria-label="Turn token detail" />
          <div className="turn-footer-detail-copy">{ledger}</div>
        </details>
      )}
    </div>
  );
}
