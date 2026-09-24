// One-click reopen for a recovered transcript. Renders the daemon's
// `resumable` verdict, never re-derives it, and never resumes by itself.
import { useState } from "react";
import { sessionResume } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import type { Session } from "../../types/ipc";

export function RecoveredSessionBar({
  session,
  onReopened,
  onResumeFailed,
}: {
  session: Session | null;
  onReopened: (session: Session) => void;
  /** A resume attempt failed. The parent re-reads the roster so this bar
   * renders the daemon's current verdict, never the stale row it held. */
  onResumeFailed?: () => void;
}) {
  const [resuming, setResuming] = useState(false);
  const [error, setError] = useState<ErrorSentence | null>(null);
  if (session === null) return null;
  const recovered = session.state.type === "recovered";
  const resumable = session.resumable === true;
  if (!recovered && !resumable) return null;

  const reopen = (): void => {
    if (resuming || !resumable) return;
    setResuming(true);
    setError(null);
    void (async () => {
      try {
        const result = await sessionResume(session.id);
        if (result.type === "resumed") {
          onReopened(result.session);
        } else if (result.type === "failed") {
          setError({ sentence: result.message, detail: null });
          onResumeFailed?.();
        } else {
          setError({ sentence: "This session does not support resume.", detail: null });
          onResumeFailed?.();
        }
      } catch (cause) {
        setError(errorSentence(cause));
        onResumeFailed?.();
      } finally {
        setResuming(false);
      }
    })();
  };

  if (!resumable) {
    return (
      <div
        className="workspace-session-error workspace-session-notice"
        role="status"
        data-testid="recovered-unresumable"
      >
        <span className="workspace-session-error-text">
          This transcript is read-only. Resume is not available for this session.
        </span>
      </div>
    );
  }

  return (
    <div
      className="workspace-session-error workspace-session-notice"
      role="status"
      data-testid="recovered-reopen-bar"
    >
      <span className="workspace-session-error-text">
        This session has no running process. The transcript is from the journal and is read-only.
      </span>
      <button
        type="button"
        className="workspace-secondary-action"
        title="Reopen this session"
        disabled={resuming}
        onClick={reopen}
      >
        {resuming ? "Reopening…" : "Reopen"}
      </button>
      {error !== null ? (
        <span className="workspace-session-error-text" role="alert">
          <ErrorText sentence={error.sentence} detail={error.detail} id="recovered-bar-error" />
        </span>
      ) : null}
    </div>
  );
}
