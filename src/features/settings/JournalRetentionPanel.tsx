import { useCallback, useEffect, useRef, useState } from "react";
import { journalRetentionGet, journalRetentionSet, journalUsage } from "../../lib/tauri";
import type { JournalRetention, JournalUsage, RetentionPatch } from "../../types/ipc";
import { useTrackedRequest } from "../../lib/trackedRequest";
import { formatCount } from "../../lib/format";
import { isImeComposition } from "../../lib/imeComposition";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import { SettingsAdvanced, SettingsRow, SettingsSection } from "./rows";
import "./diagnostics.css";

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
  const [actionError, setActionError] = useState<ErrorSentence | null>(null);
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
          setActionError(errorSentence(error));
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
      ? { sentence: usageRequest.state.message, detail: usageRequest.state.detail }
      : retentionRequest.state.status === "error"
        ? { sentence: retentionRequest.state.message, detail: retentionRequest.state.detail }
        : null;
  const error: ErrorSentence | null =
    validationError !== null
      ? { sentence: validationError, detail: null }
      : (actionError ?? readError);

  return (
    <div className="retention-panel">
      {error && (
        <div className="settings-retention-alert" role="alert">
          <ErrorText sentence={error.sentence} detail={error.detail} id="journal-retention-error" />
        </div>
      )}
      {usage && (
        <div className="retention-summary" aria-label="Transcript history usage">
          <SettingsSection label="Transcript history">
            <SettingsRow
              title="Total saved bytes"
              control={<span>{formatCount(usage.totalBytes)} bytes</span>}
            />
            <SettingsRow
              title="Saved sessions"
              control={<span>{formatCount(usage.sessionCount)}</span>}
            />
            {usage.unreclaimable.bytesOver > 0 && (
              <SettingsRow
                title="Bytes over an unreclaimable limit"
                control={
                  <span className="settings-value-danger">
                    {formatCount(usage.unreclaimable.bytesOver)} bytes
                  </span>
                }
              />
            )}
            {usage.unreclaimable.sessionsOver > 0 && (
              <SettingsRow
                title="Sessions over an unreclaimable limit"
                control={
                  <span className="settings-value-danger">
                    {formatCount(usage.unreclaimable.sessionsOver)}
                  </span>
                }
              />
            )}
            {usage.unreclaimable.agedOut > 0 && (
              <SettingsRow
                title="Sessions past an unreclaimable age"
                control={
                  <span className="settings-value-danger">
                    {formatCount(usage.unreclaimable.agedOut)}
                  </span>
                }
              />
            )}
            {blockedReasons.length > 0 && (
              <p className="settings-status">
                Retention is blocked because {blockedReasons.join(" and ")}.
              </p>
            )}
          </SettingsSection>
        </div>
      )}
      {retention && (
        <div className="retention-limits">
          <SettingsSection label="Retention limits">
            {RETENTION_FIELDS.map((field) => (
              <SettingsRow
                key={field}
                title={FIELD_LABELS[field]}
                description={retention[field].source}
                control={
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
                      if (isImeComposition(event.nativeEvent)) return;
                      if (event.key === "Enter") {
                        event.preventDefault();
                        commitField(field, values[field]);
                        event.currentTarget.blur();
                      }
                    }}
                  />
                }
              />
            ))}
          </SettingsSection>
          <SettingsAdvanced>
            <p>See how much journal history is saved and choose its retention limits.</p>
            <p>
              Enter 0 for no limit. An empty or invalid field is rejected; it never silently
              disables a limit.
            </p>
            <p>Lowering a limit takes effect immediately and can delete history.</p>
          </SettingsAdvanced>
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
