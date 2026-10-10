/**
 * Whether a tool call failed. Only the provider's own "failed" status counts:
 * a non-zero exit code alone does not, because `git diff` exits 1 when there
 * are differences and `grep` exits 1 when nothing matches. Such a call keeps
 * its normal output and shows its code as a muted "exit N".
 */
export function isFailedToolCall(call: { status: string; exitCode?: number }): boolean {
  if (call.status.toLowerCase() !== "failed") return false;
  return call.exitCode === undefined || call.exitCode === 0;
}
