/**
 * Whether a tool call failed: the provider's own "failed" status is the verdict,
 * whatever its exit code. An exit code alone is never a verdict: a call the
 * provider reports as completed with exit 1 (`git diff`, `grep` with no match)
 * keeps its normal output and shows only a muted "exit 1".
 */
export function isFailedToolCall(call: { status: string; exitCode?: number }): boolean {
  return call.status.toLowerCase() === "failed";
}
