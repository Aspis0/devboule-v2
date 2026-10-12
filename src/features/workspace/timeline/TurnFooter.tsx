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
 * The turn's billed cost: absent where the provider reports none, and dropped
 * for pi, whose figure is its own estimate from a static price list rather
 * than what the provider charged.
 */
function costCopy(finished: AgentFinished, providerId: string | undefined): string | null {
  const costUsd = finished.usage?.costUsd;
  if (costUsd === undefined || providerId === "pi") return null;
  return usdCopy(costUsd);
}

/**
 * The turn's details, in the order a reader asks: why it stopped when that was
 * abnormal, the token ledger, then what it cost.
 */
function detailCopy(finished: AgentFinished, cost: string | null): string | null {
  const usage = finished.usage;
  if (usage === undefined) return null;
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
  const cost = costCopy(finished, providerId);
  const reason = stopReason(finished);
  const detail = detailCopy(finished, cost);
  // A stop word on the row or a cost behind it is what earns the disclosure. A
  // normal end whose ledger holds only token counts draws no footer: the arrow
  // would stand alone under the reply, opening nothing the row asked for.
  const disclosure = detail !== null && (reason !== null || cost !== null);
  if (reason === null && !disclosure) return null;
  return (
    <div className="turn-footer">
      {reason === null ? null : (
        <span className="turn-footer-stop" title={`Stopped: ${reason}`}>
          stopped
        </span>
      )}
      {disclosure ? (
        <details className="turn-footer-detail">
          <summary className="turn-footer-detail-trigger" aria-label="Turn details" />
          <div className="turn-footer-detail-copy">{detail}</div>
        </details>
      ) : null}
    </div>
  );
}
