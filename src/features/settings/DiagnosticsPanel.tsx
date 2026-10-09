import { useCopyFeedback } from "../../lib/useCopyFeedback";
import { Component, type ReactNode, useEffect, useRef, useState } from "react";
import { daemonDiagnostics } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import { SettingsAdvanced, SettingsRow, SettingsSection } from "./rows";
import type { DaemonDiagnostics } from "../../types/ipc";
import "./diagnostics.css";

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
  const {
    journalStats,
    journalError,
    logError,
    journalSchemaVersion,
    journalFileBytes,
    ...counters
  } = source;
  const journal: DiagnosticsRecord = isRecord(journalStats)
    ? { ...journalStats }
    : { journalStats };
  journal.journalError = journalError;
  journal.logError = logError;
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
    <SettingsSection label={title}>
      {entries.length === 0 ? (
        // A blank section reads as "there is no problem". When a section carries
        // nothing the frontend recognises — the most likely symptom of a
        // wire-shape mismatch — it must say so.
        <p className="settings-status">(no data for this section)</p>
      ) : (
        entries.map(([key, value]) => (
          <SettingsRow
            key={key}
            title={humanizeKey(key)}
            control={<span>{formatValue(value)}</span>}
          />
        ))
      )}
    </SettingsSection>
  );
}

function ProviderRow({ row, index }: { row: unknown; index: number }) {
  const entries = sortedEntries(row);
  const name = entries.find(([key]) => key === "id" || key === "name");
  const fields = entries.filter((entry) => entry !== name);
  return (
    <SettingsRow
      title={name === undefined ? `Provider ${index + 1}` : formatValue(name[1])}
      control={
        <span>
          {fields.map(([key, value]) => `${humanizeKey(key)} ${formatValue(value)}`).join(" · ")}
        </span>
      }
    />
  );
}

interface DiagnosticsErrorBoundaryProps {
  children: ReactNode;
}

const SAFETY_NOTE =
  "This report is numbers and versions about the app itself. It is already " +
  "redacted by the daemon: no secrets, no conversation content, no session " +
  "titles, and paths with the home directory redacted. Copying it is safe — " +
  "paste it straight into an issue.";

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
        <div id="settings-panel-diagnostics">
          <div role="alert">
            <p className="settings-error">Could not render the diagnostics.</p>
            <ErrorText
              sentence={errorSentence(this.state.error).sentence}
              detail={errorSentence(this.state.error).detail}
              id="diagnostics-boundary-error"
            />
            <button
              type="button"
              className="diagnostics-boundary-retry diagnostics-retry"
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
  const [error, setError] = useState<ErrorSentence | null>(null);
  const feedback = useCopyFeedback({
    resetAfterMs: (outcome) => (outcome === "copied" ? 2000 : null),
  });
  const copyState = feedback.stateFor("diagnostics");
  const activeLoadRef = useRef<(() => void) | null>(null);

  function stopActiveLoad(): void {
    activeLoadRef.current?.();
    activeLoadRef.current = null;
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
        setError(errorSentence(cause));
      },
    );
  }

  useEffect(() => {
    startLoad();
    return () => {
      stopActiveLoad();
    };
  }, []);

  function retry(): void {
    stopActiveLoad();
    setError(null);
    setReport(null);
    startLoad();
  }

  async function copyReport(): Promise<void> {
    if (report !== null) await feedback.copy("diagnostics", formatDiagnostics(report));
  }

  if (error !== null) {
    return (
      <div id="settings-panel-diagnostics">
        <div role="alert">
          <p className="settings-error">Could not load the diagnostics.</p>
          <ErrorText sentence={error.sentence} detail={error.detail} id="diagnostics-load-error" />
          <button type="button" className="diagnostics-retry" onClick={retry}>
            Try again
          </button>
        </div>
      </div>
    );
  }

  if (report === null) {
    return (
      <div id="settings-panel-diagnostics">
        <p className="settings-status">Loading the diagnostics…</p>
      </div>
    );
  }

  if (isReportEmpty(report)) {
    return (
      <div id="settings-panel-diagnostics">
        <p className="settings-status">The daemon answered, but sent no diagnostics data.</p>
      </div>
    );
  }

  const source = asRecord(report);
  const { health, journal } = splitHealth(source.health);
  const providers = Array.isArray(source.providers) ? source.providers : [];

  return (
    <div id="settings-panel-diagnostics">
      <SettingsRow
        title="Report"
        control={
          <div className="settings-choices">
            <button
              type="button"
              className="diagnostics-copy settings-device-action"
              onClick={() => void copyReport()}
            >
              Copy
            </button>
            {copyState === "copied" ? <span className="diagnostics-copy-note">Copied.</span> : null}
          </div>
        }
      />
      {copyState === "failed" ? (
        <p className="settings-status diagnostics-copy-failed">
          Copying failed — open Advanced and copy the text manually.
        </p>
      ) : null}
      <RecordSection title="Daemon" record={source.daemon} />
      <RecordSection title="Health" record={health} />
      <RecordSection title="Journal" record={journal} />
      <RecordSection title="Sessions" record={source.sessions} />
      {providers.length > 0 ? (
        <SettingsSection label="Providers">
          {providers.map((row, index) => (
            <ProviderRow key={index} row={row} index={index} />
          ))}
        </SettingsSection>
      ) : null}
      <RecordSection title="Environment" record={source.environment} />
      <SettingsAdvanced>
        <p>{SAFETY_NOTE}</p>
        <pre className="diagnostics-text">{formatDiagnostics(report)}</pre>
      </SettingsAdvanced>
    </div>
  );
}
