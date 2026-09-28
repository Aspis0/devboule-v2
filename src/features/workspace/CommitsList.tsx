import { useEffect, useState } from "react";
import type { WorkspaceGitLog } from "../../types/ipc";
import type { ErrorSentence } from "../../lib/errorSentence";
import { relativeTime } from "../../lib/relativeTime";
import { ErrorText } from "../../components/ErrorText";

/**
 * The Commits half of the Changes panel: the branch's own commits — what
 * Paseo's section shows (`commits-section.tsx:61-70` filters the base half
 * out; this panel copies the filter, Paseo's function with our look). Rows
 * are not controls: opening a commit's diff needs a tab kind and a
 * commit-diff read that do not exist yet. The subject and the author carry
 * `title` for the text their ellipsis cuts.
 */
export function CommitsList({
  log,
  failure,
}: {
  log: WorkspaceGitLog | null;
  failure: ErrorSentence | null;
}) {
  // One clock for the list's relative times, ticked at the resolution a
  // label can change; the 30 s read re-renders it besides.
  const [nowMs, setNowMs] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNowMs(Date.now()), 10_000);
    return () => window.clearInterval(timer);
  }, []);

  if (log === null && failure === null) {
    return (
      <div className="workspace-commits-state" role="status">
        Loading commits…
      </div>
    );
  }

  // The branch's own commits, in the order their author dates give them:
  // the wire carries only `%aI`, and a time column that must not jump
  // backwards is sorted by the date it prints. The sort is stable, so
  // equal dates keep the wire's order.
  const ahead =
    log !== null
      ? log.commits
          .filter((commit) => !commit.isOnBase)
          .sort((a, b) => Date.parse(b.authorDate) - Date.parse(a.authorDate))
      : [];

  // The wire's refusal arm: an empty list WITH a sentence. A cut-short
  // list carries commits and takes the branch below; a refusal carries
  // none.
  const refusal = log !== null && log.commits.length === 0 && log.error !== null ? log.error : null;

  return (
    <>
      {/* A live failure wins: the cell can hold an older refusal as its
          reply while a newer read throws, and the present-tense problem
          must not hide behind the stale one. */}
      {failure !== null ? (
        <div className="workspace-changes-error" role="alert">
          <ErrorText
            sentence={failure.sentence}
            detail={failure.detail}
            id="changes-commits-error"
          />
        </div>
      ) : null}
      {refusal !== null ? (
        <div className="workspace-changes-error" role="alert">
          {refusal}
        </div>
      ) : null}
      {/* A refusal is the whole answer for an empty list — the alert
          above already said it, so no empty-state sentence beside it. */}
      {log === null || refusal !== null ? null : ahead.length === 0 ? (
        <div className="workspace-commits-state">{emptySentence(log)}</div>
      ) : (
        <>
          <div className="workspace-commits">
            {ahead.map((commit) => (
              <div className="workspace-commits-row" key={commit.sha}>
                <span className="workspace-commits-sha" title={commit.sha}>
                  {commit.shortSha}
                </span>
                <span className="workspace-commits-subject" title={commit.subject}>
                  {commit.subject}
                </span>
                <span className="workspace-commits-meta">
                  <span className="workspace-commits-author" title={commit.authorName}>
                    {commit.authorName}
                  </span>
                  <span className="workspace-commits-time">
                    {relativeTime(Date.parse(commit.authorDate), nowMs)}
                  </span>
                </span>
              </div>
            ))}
          </div>
          {/* A cut-short list is flagged, not dropped: the rows above are
              the newest commits, and this sentence says the oldest are
              missing — the list stands beside it, never instead of it. */}
          {log.error !== null ? <div className="workspace-commits-note">{log.error}</div> : null}
        </>
      )}
    </>
  );
}

/**
 * The empty answer's sentence: the wire's base ref when it names one.
 * Without one there is nothing to show — and that is true for both of
 * the daemon's causes for the shape (a repository with no commit yet, a
 * detached HEAD outside a rebase), which the wire does not tell apart.
 */
function emptySentence(log: WorkspaceGitLog): string {
  return log.baseRef !== null ? `No commits ahead of ${log.baseRef}` : "No commits to show";
}
