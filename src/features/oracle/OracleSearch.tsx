import { useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import type {
  OracleIndexStats,
  OracleModelStatus,
  OracleResult,
  OracleSearchResponse,
} from "../../types/ipc";
import type { TrackedRequestState } from "./oracleRequests";
import { formatCount, resultLineCount, totalReadLines } from "./oracleUtils";

interface OracleSearchProps {
  searchState: TrackedRequestState<OracleSearchResponse>;
  submittedQuery: string | null;
  stats: OracleIndexStats | null;
  indexIsEmpty: boolean;
  reranker: OracleModelStatus | null;
  onSearch: (query: string) => void;
  onRetryReranker: () => void;
}

export function OracleSearch({
  searchState,
  submittedQuery,
  stats,
  indexIsEmpty,
  reranker,
  onSearch,
  onRetryReranker,
}: OracleSearchProps) {
  const [query, setQuery] = useState("");
  const results = searchState.status === "ready" ? searchState.value.results : [];
  const totalResultLines = totalReadLines(results);

  function submitQuery() {
    const trimmedQuery = query.trim();
    if (trimmedQuery) onSearch(trimmedQuery);
  }

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    submitQuery();
  }

  function handleKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      event.preventDefault();
      submitQuery();
    }
  }

  return (
    <section className="oracle-query-surface" aria-labelledby="oracle-query-title">
      <div className="oracle-ready-intro">
        <div>
          <div className="oracle-eyebrow">Ready to ask</div>
          <h3 id="oracle-query-title">What do you need to find?</h3>
          <p>
            Ask in natural language. Oracle returns the source spans worth opening, not a generated
            answer.
          </p>
        </div>
        <RerankerStatus status={reranker} onRetry={onRetryReranker} />
      </div>

      <form className="oracle-search" onSubmit={handleSubmit}>
        <span className="oracle-search-mark" aria-hidden="true">
          ?
        </span>
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="e.g. where is the workspace root resolved?"
          aria-label="Ask Oracle a question"
        />
        <button
          className="oracle-button oracle-button-primary"
          type="submit"
          disabled={searchState.status === "loading" || !query.trim()}
        >
          {searchState.status === "loading" ? "Finding…" : "Find pointers"}
        </button>
      </form>

      <section className="oracle-results-card" aria-labelledby="oracle-results-title">
        <div className="oracle-results-heading">
          <div>
            <div className="oracle-eyebrow">Ranked code pointers</div>
            <h3 id="oracle-results-title">
              {submittedQuery ? `Results for “${submittedQuery}”` : "Your results will appear here"}
            </h3>
          </div>
          {searchState.status === "ready" && results.length > 0 && (
            <div
              className="oracle-reading-cost"
              aria-label={`${formatCount(totalResultLines)} lines to read`}
            >
              <strong>{formatCount(totalResultLines)}</strong>
              <span>lines to read</span>
            </div>
          )}
        </div>
        <p className="oracle-results-note">
          Each pointer is a file and line range to inspect. Overlapping ranges count once in the
          reading cost.
        </p>
        {searchState.status === "idle" && (
          <div className="oracle-state-message" role="status" aria-live="polite">
            Try a question about a symbol, file, or relationship in this folder.
          </div>
        )}
        {searchState.status === "loading" && (
          <div className="oracle-state-message" role="status" aria-live="polite">
            Searching the local index…
          </div>
        )}
        {searchState.status === "error" && (
          <div className="oracle-error-message" role="alert">
            {searchState.message}
          </div>
        )}
        {searchState.status === "ready" && results.length === 0 && (
          <div className="oracle-empty-state" role="status" aria-live="polite">
            {indexIsEmpty ? (
              <>
                <strong>This index is empty.</strong> Nothing can match until you index the folder.
              </>
            ) : (
              <>
                <strong>No source spans matched.</strong> The index has{" "}
                {stats ? `${formatCount(stats.indexed_files)} indexed files` : "indexed files"}; try
                a broader phrase, a symbol name, or a file path.
              </>
            )}
          </div>
        )}
        {searchState.status === "ready" && results.length > 0 && (
          <ol className="oracle-result-list" aria-label="Ranked Oracle results" aria-live="polite">
            {results.map((result, index) => (
              <OracleResultRow
                key={`${result.path}:${result.line_start}:${result.line_end}:${index}`}
                result={result}
                index={index}
              />
            ))}
          </ol>
        )}
      </section>
    </section>
  );
}

function OracleResultRow({ result, index }: { result: OracleResult; index: number }) {
  const lineCount = resultLineCount(result);
  return (
    <li
      className="oracle-result"
      tabIndex={0}
      aria-label={`${result.path}, lines ${result.line_start} to ${result.line_end}`}
    >
      <div className="oracle-result-heading">
        <span className="oracle-result-rank">#{String(index + 1).padStart(2, "0")}</span>
        <code className="oracle-result-path">{result.path}</code>
        <span className="oracle-result-range">
          lines {result.line_start}–{result.line_end}
        </span>
      </div>
      <pre className="oracle-result-snippet">
        <code>{result.snippet}</code>
      </pre>
      <div className="oracle-result-reason" aria-label="Why this result was returned">
        <span className="oracle-result-cost">{lineCount} lines to read</span>
        {result.match_type && <span>match {result.match_type}</span>}
        {result.symbol_name && <span>symbol {result.symbol_name}</span>}
      </div>
    </li>
  );
}

export function RerankerStatus({
  status,
  onRetry,
}: {
  status: OracleModelStatus | null;
  onRetry: () => void;
}) {
  if (!status) {
    return (
      <div className="oracle-reranker-status oracle-reranker-status-muted">
        Reranker status unavailable · dense only
      </div>
    );
  }

  if (status.state === "ready") {
    return (
      <div className="oracle-reranker-status oracle-reranker-status-ready" role="status">
        <span className="oracle-status-dot" aria-hidden="true" />
        Reranker active · candidates reordered
      </div>
    );
  }

  if (status.state === "downloading") {
    return (
      <div className="oracle-reranker-status oracle-reranker-status-progress" role="status">
        <span className="oracle-status-dot" aria-hidden="true" />
        Reranker downloading · about 5 MB
      </div>
    );
  }

  return (
    <div className="oracle-reranker-status oracle-reranker-status-warning" role="alert">
      <span>
        <span className="oracle-status-dot" aria-hidden="true" />
        Reranker unavailable · dense only
        <small>Audit measured about 4% lower recall without reranking.</small>
      </span>
      <button className="oracle-inline-action" type="button" onClick={onRetry}>
        Retry
      </button>
    </div>
  );
}
