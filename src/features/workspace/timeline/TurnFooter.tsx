import type { AgentFinished } from "../../../lib/agentSession";
import { usdCopy } from "../../../lib/format";
import "./TurnFooter.css";

/** The two facts a glance needs: why the turn stopped and what it cost. The model is on the composer. */
function shortCopy(finished: AgentFinished): string | null {
  const details = [
    finished.stopReason ? `stopped: ${finished.stopReason}` : null,
    finished.usage?.costUsd === undefined ? null : usdCopy(finished.usage.costUsd),
  ].filter((part): part is string => part !== null);
  return details.length > 0 ? details.join(" · ") : null;
}

/** The token ledger, which is the same count spelled out in five places. */
function detailCopy(finished: AgentFinished): string | null {
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
  ].filter((part): part is string => part !== null);
  return details.length > 0 ? details.join(" · ") : null;
}

/**
 * The finished turn's metadata, one line and one disclosure.
 *
 * The line is what a reader glances at; the ledger behind it answers "what did
 * that turn spend", and it is a <details> so the keyboard drives it without a
 * key handler and a screen reader gets the native disclosure for free.
 * A normally finished turn takes no row at all: the reply speaks for
 * itself and the context popover keeps the spend.
 */
export function TurnFooter({ finished }: { finished: AgentFinished | null }) {
  if (
    finished === null ||
    finished.stopReason === "completed" ||
    finished.stopReason === "end_turn"
  )
    return null;
  const summary = shortCopy(finished);
  if (summary === null) return null;
  const ledger = detailCopy(finished);
  return (
    <div className="turn-footer">
      <div className="turn-footer-line">{summary}</div>
      {ledger === null ? null : (
        <details className="turn-footer-detail">
          <summary className="turn-footer-detail-trigger" aria-label="Turn token detail" />
          <div className="turn-footer-detail-copy">{ledger}</div>
        </details>
      )}
    </div>
  );
}
