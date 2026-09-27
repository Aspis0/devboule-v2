import type { AgentSessionState } from "../../../lib/agentSession";
import type { SessionState } from "../../../types/ipc";

function formatElapsed(elapsedMs: number): string {
  const minutes = Math.floor(elapsedMs / 60_000);
  if (minutes > 0) return `${minutes} minute${minutes === 1 ? "" : "s"}`;
  const seconds = Math.floor(elapsedMs / 1_000);
  return `${seconds} second${seconds === 1 ? "" : "s"}`;
}

export function paneHeaderStatus(
  observed: SessionState | null | undefined,
  elapsedMs: number | null | undefined,
  agent: AgentSessionState,
): { copy: string; tone: "green" | "terracotta" | "border" } {
  const type = observed?.type ?? null;
  if (type === "ended" || type === "recovered") {
    return { copy: "Finished", tone: "terracotta" };
  }
  if (type === "silent") {
    return {
      copy: typeof elapsedMs === "number" ? `Silent for ${formatElapsed(elapsedMs)}` : "Silent",
      tone: "border",
    };
  }
  if (agent.status === "error") return { copy: "Needs attention", tone: "terracotta" };
  if (agent.status === "closed") return { copy: "Finished", tone: "terracotta" };
  if (agent.status === "running") return { copy: "Working…", tone: "green" };
  if (type === "live") return { copy: "Live", tone: "green" };
  return { copy: "Connecting…", tone: "border" };
}
