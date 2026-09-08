import { Component, type ReactNode, useEffect, useRef, useState } from "react";
import { daemonDiagnostics, reasonFromCause } from "../../lib/tauri";
import type { DaemonDiagnostics } from "../../types/ipc";
import { SettingsHeading } from "./SettingsSurface";

type DiagnosticsRecord = Record<string, unknown>;

function isRecord(value: unknown): value is DiagnosticsRecord {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function asRecord(value: unknown): DiagnosticsRecord {
  return isRecord(value) ? value : {};
}

/**
 * Human label for a report key: `errorsTotal` and `errors_total` both read
 * "errors total". Used in the cards and in the copied text so the block is
 * skimmable while staying deterministic.
 */
export function humanizeKey(key: string): string {
  return key
    .replace(/[_-]+/g, " ")
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase();
}

/** Entries of one record, sorted by key, nulls dropped — the determinism base. */
function sortedEntries(record: unknown): Array<[string, unknown]> {
  return Object.entries(asRecord(record))
    .filter(([, value]) => value !== null && value !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
}

function stableValue(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map((item) => stableValue(item)).join(", ")}]`;
  }
  if (isRecord(value)) {
    const fields = Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${stableValue(value[key])}`);
    return `{${fields.join(",")}}`;
  }
  if (typeof value === "string") return value;
  if (value === null) return "null";
  return String(value);
}

function formatValue(value: unknown): string {
  return stableValue(value);
}

/**
 * Health carries journal facts on the wire. Split those facts into a derived
 * Journal section so both the health counters and the nested stats remain
 * visible without ever coercing an object to `[object Object]`.
 */
function splitHealth(health: unknown): { health: DiagnosticsRecord; journal: DiagnosticsRecord } {
  const source = asRecord(health);
  const { journalStats, journalError, journalSchemaVersion, journalFileBytes, ...counters } =
    source;
  const journal: DiagnosticsRecord = isRecord(journalStats)
    ? { ...journalStats }
    : { journalStats };
  journal.journalError = journalError;
  journal.journalFileBytes = journalFileBytes;
  journal.journalSchemaVersion = journalSchemaVersion;
  return { health: counters, journal };
}

/** The top-level section guard used before any report is rendered. */
function isUsableDiagnosticsReport(value: unknown): value is DaemonDiagnostics {
  const report = asRecord(value);
  return (
    isRecord(report.daemon) &&
    isRecord(report.health) &&
    isRecord(report.sessions) &&
    Array.isArray(report.providers) &&
    isRecord(report.environment)
  );
}

function requireDiagnosticsReport(value: unknown): DaemonDiagnostics {
  if (!isUsableDiagnosticsReport(value)) {
    throw new Error("The daemon returned an invalid diagnostics report.");
  }
  return value;
}

/** Start one cancellable daemon request; callers cancel it on unmount/retry. */
export function loadDiagnostics(
  onReport: (report: DaemonDiagnostics) => void,
  onError: (cause: unknown) => void,
): () => void {
  let cancelled = false;
  void daemonDiagnostics()
    .then((next) => {
      const checked = requireDiagnosticsReport(next);
      if (!cancelled) onReport(checked);
    })
    .catch((cause: unknown) => {
      if (!cancelled) onError(cause);
    });
  return () => {
    cancelled = true;
  };
}

/**
 * Renders the report as a readable, deterministic text block for an issue:
 * fixed section order, keys sorted inside each section, one `key: value` per
 * line. Two reports from the same state produce identical text.
 */
export function formatDiagnostics(report: DaemonDiagnostics): string {
  const source = asRecord(report);
  const lines: string[] = ["devboule diagnostics"];

  const pushRecord = (name: string, record: unknown): void => {
    lines.push(`== ${name} ==`);
    const entries = sortedEntries(record);
    if (entries.length === 0) {
      lines.push("(none)");
      return;
    }
    for (const [key, value] of entries) {
      lines.push(`${humanizeKey(key)}: ${formatValue(value)}`);
    }
  };

  const { health, journal } = splitHealth(source.health);
  pushRecord("daemon", source.daemon);
  pushRecord("health", health);
  pushRecord("journal", journal);
  pushRecord("sessions", source.sessions);

  lines.push("== providers ==");
  const providers = Array.isArray(source.providers) ? source.providers : [];
  if (providers.length === 0) {
    lines.push("(none)");
  } else {
    for (const row of providers) {
      const entries = sortedEntries(row);
      const summary = entries
        .map(([key, value]) => `${humanizeKey(key)}: ${formatValue(value)}`)
        .join(", ");
      lines.push(`- ${summary || "(no data)"}`);
    }
  }

  pushRecord("environment", source.environment);
  return lines.join("\n");
}

/** True when the daemon answered but had nothing to report in any section. */
function isReportEmpty(report: DaemonDiagnostics): boolean {
  const source = asRecord(report);
  const { health, journal } = splitHealth(source.health);
  const daemonEntries = sortedEntries(source.daemon).filter(([, value]) => value !== "");
  const providers = Array.isArray(source.providers) ? source.providers : [];
  return (
    daemonEntries.length === 0 &&
    sortedEntries(health).length === 0 &&
    sortedEntries(journal).length === 0 &&
    sortedEntries(source.sessions).length === 0 &&
    providers.length === 0 &&
    sortedEntries(source.environment).length === 0
  );
}

interface RecordSectionProps {
  title: string;
  record: unknown;
}

function RecordSection({ title, record }: RecordSectionProps) {
  const entries = sortedEntries(record);
  return (
    <section className="settings-card diagnostics-section">
      <h3 className="settings-card-title">{title}</h3>
      {entries.length === 0 ? (
        // A blank card in a diagnostics panel reads as "there is no problem".
        // When a section carries nothing the frontend recognises — the most
        // likely symptom of a wire-shape mismatch — it must say so.
        <p className="diagnostics-section-empty">(no data for this section)</p>
      ) : (
        <dl className="diagnostics-rows">
          {entries.map(([key, value]) => (
            <div className="diagnostics-row" key={key}>
              <dt>{humanizeKey(key)}</dt>
              <dd>{formatValue(value)}</dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  );
}

const SAFETY_NOTE =
  "This report is numbers and versions about the app itself. It is already " +
  "redacted by the daemon: no secrets, no conversation content, no session " +
  "titles, and paths with the home directory redacted. Copying it is safe — " +
  "paste it straight into an issue.";

interface DiagnosticsErrorBoundaryProps {
  children: ReactNode;
}

interface DiagnosticsErrorBoundaryState {
  error: unknown;
}

/** Last-resort containment for a future renderer bug or a new wire shape. */
export class DiagnosticsErrorBoundary extends Component<
  DiagnosticsErrorBoundaryProps,
  DiagnosticsErrorBoundaryState
> {
  state: DiagnosticsErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): DiagnosticsErrorBoundaryState {
    return { error };
  }

  render() {
    if (this.state.error !== null) {
      return (
        <div id="settings-panel-diagnostics" role="tabpanel" aria-label="Diagnostics">
          <SettingsHeading title="Diagnostics" description={SAFETY_NOTE} />
          <div className="settings-card diagnostics-error" role="alert">
            <h3 className="settings-card-title">Could not render the diagnostics</h3>
            <p>{reasonFromCause(this.state.error)}</p>
            <p>
              The diagnostics response was not understood, but the rest of the app is still
              available.
            </p>
            <button
              type="button"
              className="diagnostics-boundary-retry"
              onClick={() => this.setState({ error: null })}
            >
              Try again
            </button>
          </div>
        </div>
      );
    }
    return this.props.children;
  }
}

/** Settings panel boundary; the content owns the daemon request and retry state. */
export function DiagnosticsPanel() {
  return (
    <DiagnosticsErrorBoundary>
      <DiagnosticsPanelContent />
    </DiagnosticsErrorBoundary>
  );
}

function DiagnosticsPanelContent() {
  const [report, setReport] = useState<DaemonDiagnostics | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copyState, setCopyState] = useState<"idle" | "copied" | "failed">("idle");
  const activeLoadRef = useRef<(() => void) | null>(null);
  const copyResetTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  function stopActiveLoad(): void {
    activeLoadRef.current?.();
    activeLoadRef.current = null;
  }

  function clearCopyResetTimer(): void {
    if (copyResetTimerRef.current === null) return;
    clearTimeout(copyResetTimerRef.current);
    copyResetTimerRef.current = null;
  }

  function startLoad(): void {
    stopActiveLoad();
    activeLoadRef.current = loadDiagnostics(
      (next) => {
        activeLoadRef.current = null;
        setReport(next);
      },
      (cause) => {
        activeLoadRef.current = null;
        setError(reasonFromCause(cause));
      },
    );
  }

  useEffect(() => {
    startLoad();
    return () => {
      stopActiveLoad();
      clearCopyResetTimer();
    };
  }, []);

  function retry(): void {
    stopActiveLoad();
    setError(null);
    setReport(null);
    startLoad();
  }

  async function copyReport(): Promise<void> {
    if (report === null) return;
    clearCopyResetTimer();
    try {
      await navigator.clipboard.writeText(formatDiagnostics(report));
      setCopyState("copied");
      copyResetTimerRef.current = setTimeout(() => {
        copyResetTimerRef.current = null;
        setCopyState("idle");
      }, 2_000);
    } catch {
      // The text block below stays visible, so a blocked clipboard still has
      // a manual path.
      setCopyState("failed");
    }
  }

  if (error !== null) {
    return (
      <div id="settings-panel-diagnostics" role="tabpanel" aria-label="Diagnostics">
        <SettingsHeading title="Diagnostics" description={SAFETY_NOTE} />
        <div className="settings-card diagnostics-error" role="alert">
          <h3 className="settings-card-title">Could not load the diagnostics</h3>
          <p>{error}</p>
          <p>The daemon may not be answering right now — try again once it recovers.</p>
          <button type="button" className="diagnostics-retry" onClick={retry}>
            Try again
          </button>
        </div>
      </div>
    );
  }

  if (report === null) {
    return (
      <div id="settings-panel-diagnostics" role="tabpanel" aria-label="Diagnostics">
        <SettingsHeading title="Diagnostics" description={SAFETY_NOTE} />
        <p className="diagnostics-loading">Loading the diagnostics…</p>
      </div>
    );
  }

  if (isReportEmpty(report)) {
    return (
      <div id="settings-panel-diagnostics" role="tabpanel" aria-label="Diagnostics">
        <SettingsHeading title="Diagnostics" description={SAFETY_NOTE} />
        <p className="diagnostics-empty">The daemon answered, but sent no diagnostics data.</p>
      </div>
    );
  }

  const source = asRecord(report);
  const { health, journal } = splitHealth(source.health);
  const providers = Array.isArray(source.providers) ? source.providers : [];

  return (
    <div id="settings-panel-diagnostics" role="tabpanel" aria-label="Diagnostics">
      <SettingsHeading title="Diagnostics" description={SAFETY_NOTE} />
      <div className="diagnostics-actions">
        <button type="button" className="diagnostics-copy" onClick={() => void copyReport()}>
          Copy diagnostics
        </button>
        {copyState === "copied" ? <span className="diagnostics-copy-note">Copied.</span> : null}
        {copyState === "failed" ? (
          <span className="diagnostics-copy-note diagnostics-copy-failed">
            Copying failed — select the text below and copy it manually.
          </span>
        ) : null}
      </div>
      <div className="diagnostics-grid">
        <RecordSection title="Daemon" record={source.daemon} />
        <RecordSection title="Health" record={health} />
        <RecordSection title="Journal" record={journal} />
        <RecordSection title="Sessions" record={source.sessions} />
        {providers.length > 0 ? (
          <section className="settings-card diagnostics-section">
            <h3 className="settings-card-title">Providers</h3>
            <ul className="diagnostics-providers">
              {providers.map((row, index) => {
                const entries = sortedEntries(row);
                return (
                  <li key={index}>
                    {entries.map(([key, value]) => (
                      <span className="diagnostics-provider-field" key={key}>
                        <span className="diagnostics-provider-key">{humanizeKey(key)}</span>{" "}
                        {formatValue(value)}
                      </span>
                    ))}
                  </li>
                );
              })}
            </ul>
          </section>
        ) : null}
        <RecordSection title="Environment" record={source.environment} />
      </div>
      <pre className="diagnostics-text">{formatDiagnostics(report)}</pre>
    </div>
  );
}
