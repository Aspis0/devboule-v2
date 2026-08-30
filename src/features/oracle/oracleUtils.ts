import { isCommandError } from "../../lib/tauri";
import type {
  OracleIndexStats,
  OracleIndexStatus,
  OracleModelStatus,
  OracleResult,
  OracleWorkspace,
} from "../../types/ipc";
import type { TrackedRequestState } from "./oracleRequests";

export function normalizedLineRange(result: OracleResult): [number, number] {
  return [
    Math.min(result.line_start, result.line_end),
    Math.max(result.line_start, result.line_end),
  ];
}

export function resultLineCount(result: OracleResult): number {
  const [start, end] = normalizedLineRange(result);
  if (import.meta.env.DEV && result.line_start > result.line_end) {
    console.warn("Oracle returned an inverted line range", {
      path: result.path,
      line_start: result.line_start,
      line_end: result.line_end,
    });
  }
  return Math.max(0, end - start + 1);
}

/** The focus span clamped into the result's own range, or null when absent. */
export function focusLineRange(result: OracleResult): [number, number] | null {
  const { focus_line_start: start, focus_line_end: end } = result;
  if (typeof start !== "number" || typeof end !== "number") return null;
  const [lineStart, lineEnd] = normalizedLineRange(result);
  const from = Math.max(lineStart, Math.min(start, end));
  const to = Math.min(lineEnd, Math.max(start, end));
  return from <= to ? [from, to] : null;
}

/**
 * Split a snippet into the lines before the focus, the focus itself, and the
 * lines after it. The three parts concatenate back to the original snippet, so
 * highlighting never hides a line: the reader sees the whole chunk and the
 * suggestion inside it, and can disregard the suggestion at a glance.
 */
export function splitSnippetAtFocus(
  result: OracleResult,
): { before: string; focus: string; after: string } | null {
  const range = focusLineRange(result);
  if (!range) return null;
  const [lineStart] = normalizedLineRange(result);
  const lines = result.snippet.split("\n");
  const from = range[0] - lineStart;
  const to = range[1] - lineStart;
  if (from < 0 || from > to || from >= lines.length) return null;
  const end = Math.min(to, lines.length - 1);
  const join = (slice: string[], trailing: boolean) =>
    slice.length === 0 ? "" : slice.join("\n") + (trailing ? "\n" : "");
  return {
    before: join(lines.slice(0, from), true),
    focus: lines.slice(from, end + 1).join("\n"),
    after: end + 1 < lines.length ? "\n" + lines.slice(end + 1).join("\n") : "",
  };
}

export function formatCount(value: number): string {
  return value.toLocaleString("en-US").replaceAll(",", " ");
}

export function formatMegabytes(bytes: number): string {
  return `${(bytes / 1_000_000).toFixed(1)} MB`;
}

export function totalReadLines(results: OracleResult[]): number {
  const rangesByPath = new Map<string, Array<[number, number]>>();

  for (const result of results) {
    const ranges = rangesByPath.get(result.path) ?? [];
    ranges.push(normalizedLineRange(result));
    rangesByPath.set(result.path, ranges);
  }

  let total = 0;
  for (const ranges of rangesByPath.values()) {
    ranges.sort(([firstStart], [secondStart]) => firstStart - secondStart);
    let [start, end] = ranges[0];

    for (const [nextStart, nextEnd] of ranges.slice(1)) {
      if (nextStart <= end + 1) {
        end = Math.max(end, nextEnd);
      } else {
        total += Math.max(0, end - start + 1);
        start = nextStart;
        end = nextEnd;
      }
    }

    total += Math.max(0, end - start + 1);
  }

  return total;
}

export function commandErrorMessage(error: unknown): string {
  if (isCommandError(error) && error.message.trim()) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "Unknown Oracle error.";
}

export function isUnimplemented(error: unknown): boolean {
  return isCommandError(error) && error.code === "unimplemented";
}

export function fileCount(
  tab: "indexed" | "pending" | "stale",
  stats: OracleIndexStats | null,
): string {
  if (!stats) return "—";
  const count =
    tab === "indexed"
      ? stats.indexed_files
      : tab === "pending"
        ? stats.pending_files
        : stats.stale_files;
  return formatCount(count);
}

export function progressPercentage(status: OracleIndexStatus | null): number | null {
  if (!status || status.total_files <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((status.indexed_files / status.total_files) * 100)));
}

export function modelProgressPercentage(model: OracleModelStatus | null): number | null {
  if (!model?.bytes_total || model.bytes_total <= 0) return null;
  return Math.min(100, Math.max(0, Math.round((model.bytes_done / model.bytes_total) * 100)));
}

export type OracleStage =
  | "workspace-loading"
  | "choose-workspace"
  | "oracle-loading"
  | "models"
  | "index"
  | "indexing"
  | "incomplete"
  | "oracle-error"
  | "ready";

export type OracleErrorAction = "choose-workspace" | "retry-index" | "retry-status";

export function isOracleWorkspaceError(message: string): boolean {
  const normalized = message.toLowerCase();
  return (
    normalized.includes("oracle workspace") ||
    normalized.includes("no workspace folder") ||
    normalized.includes("workspace configuration")
  );
}

export function getOracleStage({
  workspaceRequest,
  statusRequest,
}: {
  workspaceRequest: TrackedRequestState<OracleWorkspace>;
  statusRequest: TrackedRequestState<OracleIndexStatus>;
}): OracleStage {
  if (workspaceRequest.status === "loading" || workspaceRequest.status === "idle") {
    return "workspace-loading";
  }
  if (workspaceRequest.status === "error" || !workspaceRequest.value.exists) {
    return "choose-workspace";
  }
  if (statusRequest.status === "loading" || statusRequest.status === "idle") {
    return "oracle-loading";
  }
  if (statusRequest.status === "error") return "oracle-error";

  const status = statusRequest.value;
  if (status.state === "error") return "oracle-error";
  if (status.model.state !== "ready") return "models";
  if (status.reranker?.state === "downloading" || status.reranker?.state === "missing") {
    return "models";
  }
  // The worker keeps running while it waits for available memory. Keep the
  // partial-index surface visible so the user sees why progress has stopped.
  if (status.pause_reason) return "incomplete";
  if (status.state === "indexing") return "indexing";
  if (status.state === "incomplete" || status.pending_files > 0) return "incomplete";
  if (status.indexed_files === 0) return "index";
  return "ready";
}

export function getOracleErrorAction({
  statusRequest,
  status,
}: {
  statusRequest: TrackedRequestState<OracleIndexStatus>;
  status: OracleIndexStatus | null;
}): OracleErrorAction {
  if (statusRequest.status === "error" && isOracleWorkspaceError(statusRequest.message)) {
    return "choose-workspace";
  }
  if (status?.state === "error") return "retry-index";
  return "retry-status";
}

export function isIndexEmpty(
  stats: OracleIndexStats | null,
  status: OracleIndexStatus | null,
): boolean {
  if (stats) {
    return (
      stats.indexed_files === 0 &&
      stats.indexed_chunks === 0 &&
      stats.pending_files === 0 &&
      stats.stale_files === 0
    );
  }
  return Boolean(status && status.indexed_files === 0 && status.indexed_chunks === 0);
}

export function modelStateLabel(status: OracleModelStatus | null): string {
  if (!status) return "status unavailable";
  switch (status.state) {
    case "ready":
      return "ready";
    case "downloading":
      return "downloading";
    case "missing":
      return "not downloaded";
    case "failed":
      return "download failed";
    case "cancelled":
      return "download cancelled";
    case "not_applicable":
      return "not available";
  }
}
