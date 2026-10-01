import type { Attention, Session } from "../../types/ipc";

type AttentionSession = Pick<Session, "state" | "attention">;

export function activeSessionAttention(session: AttentionSession): Attention | undefined {
  return session.state.type === "ended" ? undefined : (session.attention ?? undefined);
}

export function sessionNeedsApproval(session: AttentionSession): boolean {
  return activeSessionAttention(session)?.reason === "permission";
}

export function sessionAttentionLabel(session: AttentionSession): string | null {
  const attention = activeSessionAttention(session);
  if (attention === undefined) return null;
  if (attention.reason === "permission") return "Needs your approval";
  if (attention.reason === "finished") return "Done";
  if (attention.reason === "error") return "Failed";
  return "Needs attention";
}
