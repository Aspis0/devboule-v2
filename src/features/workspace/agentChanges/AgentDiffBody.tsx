import { ErrorText } from "../../../components/ErrorText";
import type { WorkspaceAgentFileDiff } from "../../../types/ipc";
import type { ChangesReply } from "../useWorkspaceChanges";
import { DiffNote, DiffRows } from "../inlineDiff";

/** The rows of a file's recorded diff, or the note that says why there are none. */
export function AgentDiffBody({
  diff,
  errorId,
}: {
  diff: ChangesReply<WorkspaceAgentFileDiff>;
  errorId: string;
}) {
  if (diff.reply === null) {
    return diff.failure === null ? (
      <DiffNote>Loading diff…</DiffNote>
    ) : (
      <DiffNote alert>
        <ErrorText sentence={diff.failure.sentence} detail={diff.failure.detail} id={errorId} />
      </DiffNote>
    );
  }
  if (diff.reply.status === "error") return <DiffNote alert>{diff.reply.error}</DiffNote>;
  return (
    <>
      {diff.reply.status === "truncated" ? (
        <DiffNote>Part of this diff is not kept.</DiffNote>
      ) : null}
      <DiffRows lines={diff.reply.lines} />
    </>
  );
}
