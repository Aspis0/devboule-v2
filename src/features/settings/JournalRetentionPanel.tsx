import { useCallback, useEffect, useRef, useState } from "react";
import { journalRetentionGet, journalRetentionSet, journalUsage } from "../../lib/tauri";
import type { JournalRetention, JournalUsage, RetentionPatch } from "../../types/ipc";
import { useTrackedRequest } from "../oracle/oracleRequests";
import { commandErrorMessage, formatCount } from "../oracle/oracleUtils";

const RETENTION_FIELDS = [
  "sessionMaxBytes",
  "maxBytes",
  "maxSessions",
  "maxAgeMs",
] as const satisfies readonly (keyof RetentionPatch)[];

type RetentionField = (typeof RETENTION_FIELDS)[number];

const FIELD_LABELS: Record<RetentionField, string> = {
  sessionMaxBytes: "Maximum session bytes",
  maxBytes: "Maximum journal bytes",
  maxSessions: "Maximum sessions",
  maxAgeMs: "Maximum age",
};

export function JournalRetentionPanel() {
  const usageRequest = useTrackedRequest<JournalUsage>(journalUsage, { status: "loading" }, true);
  const retentionRequest = useTrackedRequest<JournalRetention>(
    journalRetentionGet,
    { status: "loading" },
    true,
  );
  const [values, setValues] = useState<Record<RetentionField, string>>(() => emptyValues());
  const [validationError, setValidationError] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const focusedField = useRef<RetentionField | null>(null);
  const editVersions = useRef<Record<RetentionField, number>>(emptyVersions());
  const submittedVersions = useRef<Record<RetentionField, number>>(emptyVersions());
  const persistedValues = useRef<Record<RetentionField, string>>(emptyValues());

  useEffect(() => {
    if (retentionRequest.state.status !== "ready") return;
    const nextValues = valuesFromRetention(retentionRequest.state.value);
    persistedValues.current = nextValues;
    setValues((current) => {
      const focused = focusedField.current;
      return focused ? { ...nextValues, [focused]: current[focused] } : nextValues;
    });
  }, [retentionRequest.state]);

  const refreshUsage = usageRequest.run;
  const commitField = useCallback(
    (field: RetentionField, rawValue: string) => {
      const parsed = parseRetentionValue(rawValue);
      if (typeof parsed !== "number") {
        setValidationError(parsed);
        return;
      }
      const version = editVersions.current[field];
      if (submittedVersions.current[field] === version) return;
      submittedVersions.current[field] = version;
      if (String(parsed) === persistedValues.current[field]) return;

      setValidationError(null);
      setActionError(null);
      void journalRetentionSet({ [field]: parsed }).then(
        (retention) => {
          const serverValue = String(retention[field].value);
          persistedValues.current[field] = serverValue;
          if (editVersions.current[field] === version && focusedField.current !== field) {
            setValues((current) => ({ ...current, [field]: serverValue }));
          }
          setActionError(null);
          refreshUsage(false);
        },
        (error: unknown) => {
          if (editVersions.current[field] === version) {
            const restored = persistedValues.current[field];
            setValues((current) => ({ ...current, [field]: restored }));
          }
          setActionError(commandErrorMessage(error));
        },
      );
    },
    [refreshUsage],
  );

  const handleChange = useCallback((field: RetentionField, rawValue: string) => {
    editVersions.current[field] += 1;
    setValues((current) => ({ ...current, [field]: rawValue }));
    const parsed = parseRetentionValue(rawValue);
    setActionError(null);
    setValidationError(typeof parsed === "number" ? null : parsed);
  }, []);

  const usage = usageRequest.state.status === "ready" ? usageRequest.state.value : null;
  const retention = retentionRequest.state.status === "ready" ? retentionRequest.state.value : null;
  const blockedReasons = usage ? retentionBlockers(usage) : [];
  const readError =
    usageRequest.state.status === "error"
      ? usageRequest.state.message
      : retentionRequest.state.status === "error"
        ? retentionRequest.state.message
        : null;
  const error = validationError ?? actionError ?? readError;

  return (
    <div className="retention-panel">
      <div className="settings-page-heading">
        <h2>Transcript history</h2>
        <p>See how much journal history is saved and choose its retention limits.</p>
      </div>
      {error && (
        <div className="settings-retention-alert" role="alert">
          {error}
        </div>
      )}
      {usage && (
        <div className="retention-summary" aria-label="Transcript history usage">
          <div className="settings-card settings-value-row">
            <span>Total saved bytes</span>
            <span className="settings-card-value">{formatCount(usage.totalBytes)} bytes</span>
          </div>
          <div className="settings-card settings-value-row">
            <span>Saved sessions</span>
            <span className="settings-card-value">{formatCount(usage.sessionCount)}</span>
          </div>
          {usage.unreclaimable.bytesOver > 0 && (
            <div className="settings-card settings-value-row">
              <span>Bytes over an unreclaimable limit</span>
              <span className="settings-card-value settings-value-danger">
                {formatCount(usage.unreclaimable.bytesOver)} bytes
              </span>
            </div>
          )}
          {usage.unreclaimable.sessionsOver > 0 && (
            <div className="settings-card settings-value-row">
              <span>Sessions over an unreclaimable limit</span>
              <span className="settings-card-value settings-value-danger">
                {formatCount(usage.unreclaimable.sessionsOver)}
              </span>
            </div>
          )}
          {usage.unreclaimable.agedOut > 0 && (
            <div className="settings-card settings-value-row">
              <span>Sessions past an unreclaimable age</span>
              <span className="settings-card-value settings-value-danger">
                {formatCount(usage.unreclaimable.agedOut)}
              </span>
            </div>
          )}
          {blockedReasons.length > 0 && (
            <p className="retention-blocked-copy">
              Retention is blocked because {blockedReasons.join(" and ")}.
            </p>
          )}
        </div>
      )}
      {retention && (
        <div className="retention-limits">
          <div className="settings-subheading">Retention limits</div>
          <p className="retention-help">
            Enter 0 for no limit. An empty or invalid field is rejected; it never silently disables
            a limit.
          </p>
          <p className="retention-help">
            Lowering a limit takes effect immediately and can delete history.
          </p>
          {RETENTION_FIELDS.map((field) => (
            <label className="settings-card retention-limit-row" key={field}>
              <span className="settings-card-copy">
                <span className="settings-card-title">{FIELD_LABELS[field]}</span>
                <span className="settings-card-meta">{retention[field].source}</span>
              </span>
              <input
                aria-label={FIELD_LABELS[field]}
                className="retention-limit-input"
                inputMode="numeric"
                min="0"
                step="1"
                type="number"
                value={values[field]}
                onBlur={() => {
                  focusedField.current = null;
                  commitField(field, values[field]);
                }}
                onChange={(event) => handleChange(field, event.currentTarget.value)}
                onFocus={() => {
                  focusedField.current = field;
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitField(field, values[field]);
                    event.currentTarget.blur();
                  }
                }}
              />
            </label>
          ))}
        </div>
      )}
    </div>
  );
}

function emptyValues(): Record<RetentionField, string> {
  return {
    sessionMaxBytes: "",
    maxBytes: "",
    maxSessions: "",
    maxAgeMs: "",
  };
}

function emptyVersions(): Record<RetentionField, number> {
  return {
    sessionMaxBytes: 0,
    maxBytes: 0,
    maxSessions: 0,
    maxAgeMs: 0,
  };
}

function parseRetentionValue(rawValue: string): number | string {
  const trimmed = rawValue.trim();
  if (!/^\d+$/.test(trimmed)) {
    return "Enter a whole number. Enter 0 to disable a limit.";
  }
  const value = Number(trimmed);
  if (!Number.isSafeInteger(value)) {
    return "Enter a whole number within the supported range.";
  }
  return value;
}

function valuesFromRetention(retention: JournalRetention): Record<RetentionField, string> {
  return {
    sessionMaxBytes: String(retention.sessionMaxBytes.value),
    maxBytes: String(retention.maxBytes.value),
    maxSessions: String(retention.maxSessions.value),
    maxAgeMs: String(retention.maxAgeMs.value),
  };
}

function retentionBlockers(usage: JournalUsage): string[] {
  const blockers: string[] = [];
  if (usage.unreclaimable.bytesOver > 0) {
    blockers.push(`${formatCount(usage.unreclaimable.bytesOver)} bytes over the byte limit`);
  }
  if (usage.unreclaimable.sessionsOver > 0) {
    blockers.push(
      `${formatCount(usage.unreclaimable.sessionsOver)} sessions over the session limit`,
    );
  }
  if (usage.unreclaimable.agedOut > 0) {
    blockers.push(`${formatCount(usage.unreclaimable.agedOut)} sessions past the age limit`);
  }
  return blockers;
}
