import type { AgentFinished } from "../../../lib/agentSession";
import { usdCopy } from "../../../lib/format";
import "./TurnFooter.css";

/** Why an abnormal turn stopped. A normal end names no reason: the reply is the answer. */
function stopReason(finished: AgentFinished): string | null {
  if (!finished.stopReason || isNormalEnd(finished.stopReason)) return null;
  return finished.stopReason;
}

/** Normal ends are the ones the daemon's stop-reason mapping counts as completed. */
function isNormalEnd(stopReason: string): boolean {
  return stopReason === "stop" || stopReason === "end_turn" || stopReason === "completed";
}

/**
 * The turn's details, in the order a reader asks: why it stopped when that was
 * abnormal, the token ledger, then what it cost.
 */
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
 * The finished turn's details behind one disclosure. The row carries no words of
 * its own: a <details> keeps the keyboard driving it without a key handler, and a
 * screen reader gets the native disclosure for free.
 */
export function TurnFooter({
  finished,
  providerId,
}: {
  finished: AgentFinished | null;
  providerId?: string;
}) {
  if (finished === null) return null;
  const detail = detailCopy(finished, providerId);
  const reason = stopReason(finished);
  if (detail === null && reason === null) return null;
  return (
    <div className="turn-footer">
      {reason === null ? null : (
        <span className="turn-footer-stop" title={`Stopped: ${reason}`}>
          stopped
        </span>
      )}
      {detail === null ? null : (
        <details className="turn-footer-detail">
          <summary className="turn-footer-detail-trigger" aria-label="Turn details" />
          <div className="turn-footer-detail-copy">{detail}</div>
        </details>
      )}
    </div>
  );
}
