// Decides whether a running-status tool row was cut off by its session ending.
export const INTERRUPTED_TOOL_CLASS = "is-interrupted";
export const INTERRUPTED_TOOL_COPY = "Interrupted — session ended";

export function isToolRunningStatus(status: string): boolean {
  const normalized = status.toLowerCase();
  return normalized === "running" || normalized === "pending" || normalized === "in_progress";
}

export function isInterruptedToolStatus(status: string, transcriptEnded: boolean): boolean {
  return transcriptEnded && isToolRunningStatus(status);
}
