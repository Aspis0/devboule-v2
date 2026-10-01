// One-click reopen for a recovered transcript. Renders the daemon's
// `resumable` verdict, never re-derives it, and never resumes by itself.
import { useEffect, useRef, useState } from "react";
import { sessionResume } from "../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../lib/errorSentence";
import { ErrorText } from "../../components/ErrorText";
import { isAgentKind, type Session } from "../../types/ipc";

const UNRESUMABLE_NOTE = "This transcript is read-only. Resume is not available for this session.";

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
  // A failed Reopen fills it, a successful one empties it; the pane mounts
  // this bar keyed per session, so no other session's outcome reaches here.
  const [announcement, setAnnouncement] = useState("");
  const mounted = useRef(true);
  // The keyed remount discards this fiber while its resume may still be in
  // flight; StrictMode's simulated unmount must not leave this false.
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const resumable = session?.resumable === true;
  // Error and announcement belong to the verdict they were produced under: the
  // unresumable note is the sentence's only copy, and a repaired verdict starts clean.
  if (!resumable && (error !== null || announcement !== "")) {
    setError(null);
    setAnnouncement("");
  }
  if (session === null) return null;
  // A recovered terminal tells its story once, inside its own pane (the ended
  // banner with its close-tab action). This bar's resume verdict is about
  // agent transcripts; for a terminal it would be a second, differently
  // worded telling of the same failure.
  if (!isAgentKind(session.kind)) return null;
  const recovered = session.state.type === "recovered";
  if (!recovered && !resumable) return null;

  const reopen = (): void => {
    if (resuming || !resumable) return;
    setResuming(true);
    setError(null);
    // Each attempt gets its own transition: empty here, refilled by failure.
    setAnnouncement("");
    void (async () => {
      try {
        const result = await sessionResume(session.id);
        if (result.type === "resumed") {
          if (mounted.current) {
            setAnnouncement("");
            onReopened(result.session);
          }
          return;
        }
        const failure =
          result.type === "failed"
            ? { sentence: result.message, detail: null }
            : { sentence: "This session does not support resume.", detail: null };
        if (mounted.current) {
          setError(failure);
          setAnnouncement(UNRESUMABLE_NOTE);
        }
        // The roster refresh is harmless, so it runs even from a dead fiber.
        onResumeFailed?.();
      } catch (cause) {
        if (mounted.current) {
          setError(errorSentence(cause));
          setAnnouncement(UNRESUMABLE_NOTE);
        }
        onResumeFailed?.();
      } finally {
        if (mounted.current) {
          setResuming(false);
        }
      }
    })();
  };

  // The recovered state is a state, not a failure: the quiet note while
  // nothing is wrong, the danger block only after a failed reopen.
  const barClass =
    error === null
      ? "workspace-session-notice workspace-session-recovered"
      : "workspace-session-notice workspace-session-error";

  const verdictStatus = (
    <span className="sr-only" role="status" data-testid="recovered-verdict-status">
      {announcement}
    </span>
  );

  if (!resumable) {
    return (
      <>
        <div
          className="workspace-session-notice workspace-session-recovered"
          data-testid="recovered-unresumable"
        >
          <span className="workspace-session-notice-text">{UNRESUMABLE_NOTE}</span>
        </div>
        {verdictStatus}
      </>
    );
  }

  return (
    <>
      <div className={barClass} data-testid="recovered-reopen-bar">
        <span className="workspace-session-notice-text">
          Read-only transcript from the journal.
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
      {verdictStatus}
    </>
  );
}
