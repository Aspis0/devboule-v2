import { useEffect, useRef, useState } from 'react';
import type { FormEvent, KeyboardEvent } from 'react';
import {
  getOracleChecks,
  ORACLE_ANSWER,
  ORACLE_BACKEND,
  ORACLE_CITATIONS,
  ORACLE_CHUNK_COUNT,
  ORACLE_DATA_DIR,
  ORACLE_FILE_COUNT,
  ORACLE_FILE_TABS,
  ORACLE_FILES,
  ORACLE_INDEXED_FOLDER,
  ORACLE_INDEXING_PROGRESS,
  ORACLE_LLM,
  ORACLE_STATS,
  type OracleFileTab,
} from './mockData';
import './oracle.css';

export function OraclePanel() {
  const [oracleQuery, setOracleQuery] = useState('');
  const [oracleAnswer, setOracleAnswer] = useState(ORACLE_ANSWER);
  const [oracleStreaming, setOracleStreaming] = useState(false);
  const [providerKeyPresent, setProviderKeyPresent] = useState(true);
  const [doctorRun, setDoctorRun] = useState(false);
  const [watching, setWatching] = useState(true);
  const [jobActive, setJobActive] = useState(false);
  const [fileTab, setFileTab] = useState<OracleFileTab>('indexed');
  const streamTimerRef = useRef<number | null>(null);

  function clearOracleStream() {
    if (streamTimerRef.current !== null) {
      window.clearInterval(streamTimerRef.current);
      streamTimerRef.current = null;
    }
  }

  useEffect(() => clearOracleStream, []);

  function streamOracle() {
    clearOracleStream();
    setOracleAnswer('');
    setOracleStreaming(true);

    let index = 0;
    streamTimerRef.current = window.setInterval(() => {
      index += 3;
      setOracleAnswer(ORACLE_ANSWER.slice(0, index));

      if (index >= ORACLE_ANSWER.length) {
        clearOracleStream();
        setOracleStreaming(false);
      }
    }, 22);
  }

  function oracleAsk(event?: FormEvent<HTMLFormElement>) {
    event?.preventDefault();
    if (!oracleQuery.trim()) return;
    streamOracle();
  }

  function handleQueryKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === 'Enter') {
      event.preventDefault();
      oracleAsk();
    }
  }

  const orChecks = getOracleChecks(doctorRun, providerKeyPresent);
  const orServerLabel = jobActive ? 'indexing' : watching ? 'running' : 'starting…';
  const orWatchLabel = jobActive ? 'running' : watching ? 'watching' : 'idle';
  const orFiles2 = ORACLE_FILES[fileTab];
  const orRetrievalOnly = !providerKeyPresent;
  const orLlmLine = orRetrievalOnly
    ? ORACLE_LLM.missingLine
    : ORACLE_LLM.readyLine;
  const orLlmState = orRetrievalOnly ? 'missing api key' : 'configured';
  const orAnswerBy = orRetrievalOnly ? ORACLE_LLM.missingAnswerBy : ORACLE_LLM.readyAnswerBy;

  return (
    <div className="oracle-panel">
      <div className="settings-page-heading">
        <h2>Oracle administration</h2>
        <p>Runtime, workspace indexing &amp; health for the Devboule retrieval index.</p>
      </div>

      <div className="oracle-health" aria-label="Oracle health">
        <span className="oracle-server-state">
          <span
            className={`oracle-server-dot oracle-server-dot-${jobActive ? 'indexing' : watching ? 'running' : 'starting'}`}
            aria-hidden="true"
          />
          <span>{orServerLabel}</span>
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span className="oracle-checks" aria-label="Doctor checks">
          {orChecks.map((check) => (
            <span className="oracle-check" key={check.id} title={check.title}>
              <span className={`oracle-check-dot oracle-check-dot-${check.ok ? 'ok' : doctorRun ? 'failed' : 'idle'}`} aria-hidden="true" />
              <span>{check.id}</span>
            </span>
          ))}
        </span>
        <span className="oracle-health-divider" aria-hidden="true" />
        <span className="oracle-health-summary">{ORACLE_FILE_COUNT} files · {ORACLE_CHUNK_COUNT} chunks · {ORACLE_BACKEND}</span>
        <button className="oracle-button oracle-button-secondary oracle-doctor-button" type="button" onClick={() => setDoctorRun(true)}>
          {doctorRun ? '5/6 checks pass' : 'Run doctor'}
        </button>
      </div>

      <form className="oracle-search" onSubmit={oracleAsk}>
        <span className="oracle-search-mark" aria-hidden="true">?</span>
        <input
          value={oracleQuery}
          onChange={(event) => setOracleQuery(event.target.value)}
          onKeyDown={handleQueryKeyDown}
          placeholder="Ask the index — e.g. where the workspace root is resolved"
          aria-label="Ask the Oracle index"
        />
        <button className="oracle-button oracle-button-primary" type="submit">Ask</button>
      </form>

      <div className="oracle-answer-card">
        <div className="oracle-answer" aria-live="polite">
          {oracleAnswer}
          {oracleStreaming && <span className="oracle-caret" aria-hidden="true" />}
        </div>
        {orRetrievalOnly && (
          <div className="oracle-retrieval-banner">
            No provider key — retrieval-only answer, showing the matching code.
          </div>
        )}
        <div className="oracle-citations" aria-label="Answer citations">
          {ORACLE_CITATIONS.map((citation) => (
            <button className="oracle-citation" type="button" key={citation.label} title={citation.title}>
              {citation.label}
            </button>
          ))}
        </div>
        <div className="oracle-answer-by">
          Answer by {orAnswerBy}
        </div>
      </div>

      <div className="oracle-section-heading">
        <span>Workspace</span>
        <span>the folder Oracle indexes</span>
        <span className={`oracle-watch-badge oracle-watch-badge-${jobActive ? 'running' : watching ? 'watching' : 'idle'}`}>
          {orWatchLabel}
        </span>
      </div>

      <div className="oracle-workspace-card">
        <div className="oracle-eyebrow">Indexed folder</div>
        <div className="oracle-folder-row">
          <span className="oracle-path">{ORACLE_INDEXED_FOLDER}</span>
          <button className="oracle-button oracle-button-secondary oracle-change-button" type="button">Change</button>
        </div>
        <div className="oracle-folder-meta">→ data in <span>{ORACLE_DATA_DIR}</span></div>
        <div className="oracle-folder-ok">✓ Workspace is set — Oracle indexes this folder.</div>

        {jobActive && (
          <div className="oracle-job" aria-label="Indexing progress">
            <div className="oracle-job-heading">
              <span>Indexing… {ORACLE_INDEXING_PROGRESS.indexed} / {ORACLE_INDEXING_PROGRESS.expected}<span className="oracle-muted"> · {ORACLE_INDEXING_PROGRESS.eta}</span></span>
              <span>{ORACLE_INDEXING_PROGRESS.percentage}</span>
            </div>
            <div className="oracle-progress-track"><div className="oracle-progress-fill" /></div>
            <div className="oracle-job-note">The first batch is the slowest — the embedding model is warming up.</div>
          </div>
        )}

        <div className="oracle-stats">
          {ORACLE_STATS.slice(0, 2).map((stat) => (
            <div className="oracle-stat" key={stat.label}>
              <div className="oracle-stat-value">{stat.value}</div>
              <div className="oracle-stat-label">{stat.label}</div>
            </div>
          ))}
          <div className="oracle-stat" key="Pending">
            <div className={`oracle-stat-value${jobActive ? ' oracle-stat-value-warning' : ''}`}>{jobActive ? '902' : '0'}</div>
            <div className="oracle-stat-label">Pending</div>
          </div>
          {ORACLE_STATS.slice(2).map((stat) => (
            <div className="oracle-stat" key={stat.label}>
              <div className={`oracle-stat-value oracle-stat-value-${stat.kind}`}>{stat.value}</div>
              <div className="oracle-stat-label">{stat.label}</div>
            </div>
          ))}
        </div>

        <div className="oracle-actions">
          <button className="oracle-button oracle-button-primary" type="button" onClick={() => setJobActive(true)}>Index now</button>
          <button className="oracle-button oracle-button-secondary" type="button" onClick={() => setWatching(true)}>Watch</button>
          <button className="oracle-button oracle-button-secondary oracle-stop-button" type="button" onClick={() => { setWatching(false); setJobActive(false); }}>Stop</button>
        </div>
      </div>

      <div className="oracle-file-heading">
        <div className="oracle-file-tabs" role="tablist" aria-label="Oracle files">
          {ORACLE_FILE_TABS.map((tab) => (
            <button
              className={`oracle-file-tab${fileTab === tab.id ? ' oracle-file-tab-active' : ''}`}
              type="button"
              role="tab"
              aria-selected={fileTab === tab.id}
              key={tab.id}
              onClick={() => setFileTab(tab.id)}
            >
              <span>{tab.label}</span>
              <span>{tab.count}</span>
            </button>
          ))}
        </div>
        <span className="oracle-filter-pill">Filter files</span>
      </div>

      <div className="oracle-file-list">
        {orFiles2.map((file) => (
          <div className="oracle-file-row" key={file.path}>
            <span className="oracle-file-path">{file.path}</span>
            <span className="oracle-file-detail">{file.chunks} chunks</span>
            <span className="oracle-file-when">{file.when}</span>
          </div>
        ))}
        <div className="oracle-page-label">
          {fileTab === 'pending' ? 'Nothing pending — the watcher is caught up.' : 'Showing 100 per page'}
        </div>
      </div>

      <div className="oracle-secondary-actions">
        <button
          className="oracle-secondary-card oracle-secondary-card-button"
          type="button"
          aria-pressed={providerKeyPresent}
          title="Toggle the mock provider key"
          onClick={() => setProviderKeyPresent((present) => !present)}
        >
          <span>
            <span className="oracle-secondary-title">Oracle LLM</span>
            <span className="oracle-secondary-line">{orLlmLine}</span>
          </span>
          <span className={`oracle-secondary-state oracle-secondary-state-${providerKeyPresent ? 'ready' : 'missing'}`}>{orLlmState}</span>
          <span className="oracle-secondary-chevron" aria-hidden="true">▸</span>
        </button>
        <div className="oracle-secondary-card">
          <span>
            <span className="oracle-secondary-title">CLI Agents</span>
            <span className="oracle-secondary-line">Register the Oracle MCP server in the local Claude and Codex config.</span>
          </span>
          <span className="oracle-secondary-state oracle-secondary-state-ready">2 registered</span>
          <span className="oracle-secondary-chevron" aria-hidden="true">▸</span>
        </div>
      </div>
    </div>
  );
}
