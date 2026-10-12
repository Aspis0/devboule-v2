// A file the git list no longer shows as uncommitted: its recorded edit stands in
// for the git diff, under the caption the SPEC gives it.
import type { WorkspaceKey } from "../hosts/hostIdentity";
import { ChangeCounts } from "../ChangeCounts";
import { InlineDiff } from "../inlineDiff";
import { AgentDiffBody } from "./AgentDiffBody";
import { useAgentFileDiff } from "./useAgentFileDiff";

export const COMMITTED_CAPTION = "Committed since";

export function CommittedEdit({
  workspaceKey,
  path,
}: {
  workspaceKey: WorkspaceKey;
  path: string;
}) {
  const diff = useAgentFileDiff(workspaceKey, path);
  const reply = diff.reply;
  const counted = reply !== null && (reply.status === "ok" || reply.status === "truncated");
  const additions = counted ? reply.lines.filter((line) => line.kind === "add").length : 0;
  const deletions = counted ? reply.lines.filter((line) => line.kind === "remove").length : 0;
  return (
    <InlineDiff
      path={path}
      counts={counted ? <ChangeCounts additions={additions} deletions={deletions} /> : null}
      meta={COMMITTED_CAPTION}
    >
      <AgentDiffBody diff={diff} errorId="committed-edit-error" />
    </InlineDiff>
  );
}
