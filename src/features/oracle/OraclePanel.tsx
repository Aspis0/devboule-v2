import { useMemo, useState } from "react";
import type { FormEvent, KeyboardEvent } from "react";
import {
  getOracleChecks,
  ORACLE_DATA_DIR,
  ORACLE_FILES,
  ORACLE_FILE_TABS,
  ORACLE_HEALTH,
  ORACLE_INDEXED_FOLDER,
  ORACLE_INDEX_PROGRESS,
  ORACLE_INDEX_STATUS,
  ORACLE_SEARCH_RESPONSE,
  ORACLE_STATS,
  type OracleFileTab,
} from "./mockData";
import type { OracleResult } from "../../types/ipc";
import "./oracle.css";

const ORACLE_FILE_PANEL_ID = "oracle-file-panel";

function normalizedLineRange(result: OracleResult): [number, number] {
  return [
    Math.min(result.line_start, result.line_end),
    Math.max(result.line_start, result.line_end),
  ];
}

function resultLineCount(result: OracleResult): number {
  const [start, end] = normalizedLineRange(result);
  if (import.meta.env.DEV && result.line_start > result.line_end) {
    console.warn("Oracle returned an inverted line range", {
      path: result.path,
      line_start: result.line_start,
      line_end: result.line_end,
    });
  }
  return Math.max(0, end - start + 1);
}

function formatCount(value: number): string {
  return value.toLocaleString("en-US").replaceAll(",", " ");
}

function formatEta(seconds: number | null): string {
  if (seconds === null) return "eta unavailable";
  if (seconds < 60) return `about ${seconds}s left`;
  return `about ${Math.ceil(seconds / 60)} min left`;
}

function formatScore(score: unknown): string {
  return typeof score === "number" && Number.isFinite(score) ? score.toFixed(2) : "unavailable";
}

function totalReadLines(results: OracleResult[]): number {
  const rangesByPath = new Map<string, Array<[number, number]>>();

  for (const result of results) {
    const ranges = rangesByPath.get(result.path) ?? [];
    ranges.push(normalizedLineRange(result));
    rangesByPath.set(result.path, ranges);
  }

  let total = 0;
  for (const ranges of rangesByPath.values()) {
    ranges.sort(([firstStart], [secondStart]) => firstStart - secondStart);
    let [start, end] = ranges[0];

    for (const [nextStart, nextEnd] of ranges.slice(1)) {
      if (nextStart <= end + 1) {
        end = Math.max(end, nextEnd);
      } else {
        total += Math.max(0, end - start + 1);
        start = nextStart;
        end = nextEnd;
      }
    }

    total += Math.max(0, end - start + 1);
  }

  return total;
}

export function OraclePanel() {
  const [oracleQuery, setOracleQuery] = useState(ORACLE_SEARCH_RESPONSE.query);
  const [submittedQuery, setSubmittedQuery] = useState(ORACLE_SEARCH_RESPONSE.query);
  const [oracleResults, setOracleResults] = useState(ORACLE_SEARCH_RESPONSE.results);
  const [doctorRun, setDoctorRun] = useState(false);
  const [watching, setWatching] = useState(true);
  const [jobActive, setJobActive] = useState(ORACLE_INDEX_STATUS.state === "indexing");
  const [fileTab, setFileTab] = useState<OracleFileTab>("indexed");

  function oracleAsk(event?: FormEvent<HTMLFormElement>) {
    event?.preventDefault();
    const query = oracleQuery.trim();
    if (!query) return;

    // The mock returns a typed search response synchronously. Real IPC will replace this boundary.
    setSubmittedQuery(query);
    setOracleResults(ORACLE_SEARCH_RESPONSE.results);
  }

  function handleQueryKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      event.preventDefault();
      oracleAsk();
    }
  }

  const healthChecks = useMemo(() => getOracleChecks(doctorRun), [doctorRun]);
  const healthState = doctorRun ? "healthy" : ORACLE_HEALTH.state;
  const serverState = jobActive ? "indexing" : watching ? "running" : "stopped";
  const watchState = jobActive ? "running" : watching ? "watching" : "idle";
  const files = ORACLE_FILES[fileTab];
  const totalResultLines = useMemo(() => totalReadLines(oracleResults), [oracleResults]);
  const progressPercentage = Math.min(100, Math.max(0, ORACLE_INDEX_PROGRESS.percentage));

  return (
    <div className="oracle-panel">
      <div className="oracle-page-heading">
        <h2>Oracle pointers</h2>
        <p>Oracle finds the smallest useful code spans. It points; you read the source.</p>
      </div>

      <div className="oracle-health" aria-label="Oracle health">
        <span className="oracle-server-state">
          <span
            className={`oracle-server-dot oracle-server-dot-${serverState}`}
            aria-hidden="true"
          />
          <span>{serverState}</span>
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span className={`oracle-health-state oracle-health-state-${healthState}`}>
          health: {healthState}
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span className="oracle-checks" aria-label="Oracle health checks">
          {healthChecks.map((check) => (
            <span className="oracle-check" key={check.id} title={check.message ?? check.id}>
              <span
                className={`oracle-check-dot oracle-check-dot-${check.state}`}
                aria-hidden="true"
              />
              <span>{check.id}</span>
            </span>
          ))}
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span className="oracle-health-summary">
          {formatCount(ORACLE_STATS.indexed_files)} files ·{" "}
          {formatCount(ORACLE_STATS.indexed_chunks)} chunks · {ORACLE_STATS.backend}
        </span>
        <button
          className="oracle-button oracle-button-secondary oracle-doctor-button"
          type="button"
          onClick={() => setDoctorRun(true)}
        >
          {doctorRun ? "Doctor passed" : "Run doctor"}
        </button>
      </div>

      <form className="oracle-search" onSubmit={oracleAsk}>
        <span className="oracle-search-mark" aria-hidden="true">
          ?
        </span>
        <input
          value={oracleQuery}
          onChange={(event) => setOracleQuery(event.target.value)}
          onKeyDown={handleQueryKeyDown}
          placeholder="Find code to read — e.g. where the workspace root is resolved"
          aria-label="Search Oracle pointers"
        />
        <button className="oracle-button oracle-button-primary" type="submit">
          Find pointers
        </button>
      </form>

      <section className="oracle-results-card" aria-labelledby="oracle-results-title">
        <div className="oracle-results-heading">
          <div>
            <div className="oracle-eyebrow">Ranked code pointers</div>
            <h3 id="oracle-results-title">Results for “{submittedQuery}”</h3>
          </div>
          <div className="oracle-reading-cost" aria-label={`${totalResultLines} lines to read`}>
            <strong>{formatCount(totalResultLines)}</strong>
            <span>lines to read</span>
          </div>
        </div>
        <p className="oracle-results-note">
          No generated answer. Each result is a source span, and the line count is the context cost
          you pay to inspect it. Overlapping spans count once in the total.
        </p>
        <ol className="oracle-result-list" aria-label="Ranked Oracle results" aria-live="polite">
          {oracleResults.map((result, index) => {
            const lineCount = resultLineCount(result);
            return (
              <li
                className="oracle-result"
                key={`${result.path}:${result.line_start}:${result.line_end}:${index}`}
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
                  <span>score {formatScore(result.score)}</span>
                  {result.match_type && <span>match {result.match_type}</span>}
                  {result.symbol_name && <span>symbol {result.symbol_name}</span>}
                </div>
              </li>
            );
          })}
        </ol>
      </section>

      <div className="oracle-section-heading">
        <span>Index</span>
        <span>workspace coverage, progress, and resource ceiling</span>
        <span className={`oracle-watch-badge oracle-watch-badge-${watchState}`}>{watchState}</span>
      </div>

      <div className="oracle-workspace-card">
        <div className="oracle-eyebrow">Indexed folder</div>
        <div className="oracle-folder-row">
          <span className="oracle-path">{ORACLE_INDEXED_FOLDER}</span>
          <button
            className="oracle-button oracle-button-secondary oracle-change-button"
            type="button"
          >
            Change
          </button>
        </div>
        <div className="oracle-folder-meta">
          → data in <span>{ORACLE_DATA_DIR}</span>
        </div>
        <div className="oracle-folder-ok">✓ Workspace is set — Oracle indexes this folder.</div>

        <div className="oracle-job" aria-label="Indexing progress">
          <div className="oracle-job-heading">
            <span>
              {jobActive ? "Indexing" : "Index paused"} ·{" "}
              {formatCount(ORACLE_INDEX_PROGRESS.completed_files)} /{" "}
              {formatCount(ORACLE_INDEX_PROGRESS.total_files)} files
              <span className="oracle-muted">
                {" "}
                · {formatEta(ORACLE_INDEX_PROGRESS.eta_seconds)}
              </span>
            </span>
            <span>{progressPercentage}%</span>
          </div>
          <div
            className="oracle-progress-track"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={progressPercentage}
            aria-label="Oracle indexing progress"
          >
            <div className="oracle-progress-fill" style={{ width: `${progressPercentage}%` }} />
          </div>
          <div className="oracle-job-note">
            {jobActive
              ? `Current file: ${ORACLE_INDEX_PROGRESS.current_path ?? "waiting for the next file"}`
              : "Indexing is paused; the last reported progress is retained."}
          </div>
        </div>

        <div className="oracle-resource-cap" aria-label="Declared Oracle resource cap">
          <div>
            <div className="oracle-eyebrow">Declared resource cap</div>
            <div className="oracle-resource-copy">Background work stays within these limits.</div>
          </div>
          <div className="oracle-resource-values">
            <span>
              <strong>{ORACLE_INDEX_STATUS.resource_budget.max_cpu_percent}%</strong>
              <small>CPU</small>
            </span>
            <span>
              <strong>{formatCount(ORACLE_INDEX_STATUS.resource_budget.max_memory_mb)} MB</strong>
              <small>memory</small>
            </span>
            <span>
              <strong>{ORACLE_INDEX_STATUS.resource_budget.max_parallelism}</strong>
              <small>workers</small>
            </span>
          </div>
        </div>

        <div className="oracle-stats">
          <div className="oracle-stat">
            <div className="oracle-stat-value">{formatCount(ORACLE_STATS.indexed_files)}</div>
            <div className="oracle-stat-label">Indexed</div>
          </div>
          <div className="oracle-stat">
            <div className="oracle-stat-value">{formatCount(ORACLE_STATS.indexed_chunks)}</div>
            <div className="oracle-stat-label">Chunks</div>
          </div>
          <div className="oracle-stat">
            <div className="oracle-stat-value oracle-stat-value-warning">
              {formatCount(ORACLE_STATS.pending_files)}
            </div>
            <div className="oracle-stat-label">Pending</div>
          </div>
          <div className="oracle-stat">
            <div className="oracle-stat-value oracle-stat-value-warning">
              {formatCount(ORACLE_STATS.stale_files)}
            </div>
            <div className="oracle-stat-label">Stale</div>
          </div>
          <div className="oracle-stat">
            <div className="oracle-stat-value oracle-stat-value-ink-soft">
              {ORACLE_STATS.backend}
            </div>
            <div className="oracle-stat-label">Backend</div>
          </div>
        </div>

        <div className="oracle-actions">
          <button
            className="oracle-button oracle-button-primary"
            type="button"
            onClick={() => setJobActive(true)}
          >
            Index now
          </button>
          <button
            className="oracle-button oracle-button-secondary"
            type="button"
            onClick={() => setWatching(true)}
          >
            Watch
          </button>
          <button
            className="oracle-button oracle-button-secondary oracle-stop-button"
            type="button"
            onClick={() => {
              setWatching(false);
              setJobActive(false);
            }}
          >
            Stop
          </button>
        </div>
      </div>

      <div className="oracle-file-heading">
        <div className="oracle-file-tabs" role="tablist" aria-label="Oracle files">
          {ORACLE_FILE_TABS.map((tab) => (
            <button
              className={`oracle-file-tab${fileTab === tab.id ? " oracle-file-tab-active" : ""}`}
              type="button"
              role="tab"
              aria-selected={fileTab === tab.id}
              aria-controls={ORACLE_FILE_PANEL_ID}
              id={`oracle-file-tab-${tab.id}`}
              key={tab.id}
              onClick={() => setFileTab(tab.id)}
            >
              <span>{tab.label}</span>
              <span>{formatCount(tab.count)}</span>
            </button>
          ))}
        </div>
        <span className="oracle-filter-pill">Filter files</span>
      </div>

      <div
        id={ORACLE_FILE_PANEL_ID}
        className="oracle-file-list"
        role="tabpanel"
        aria-label="Oracle files"
      >
        {files.map((file) => (
          <div className="oracle-file-row" key={file.path}>
            <span className="oracle-file-path">{file.path}</span>
            <span className="oracle-file-detail">{file.chunks} chunks</span>
            <span className="oracle-file-when">{file.updated_at}</span>
          </div>
        ))}
        <div className="oracle-page-label">
          {fileTab === "pending"
            ? "Showing files waiting for the next index pass."
            : "Showing 100 per page"}
        </div>
      </div>
    </div>
  );
}
