import type { Attention, AttentionReason, Session } from "../../types/ipc";

/** A raise's word, the same one in every table that lists attention. */
export function attentionReasonLabel(reason: AttentionReason): string {
  if (reason === "permission") return "Needs your approval";
  if (reason === "finished") return "Done";
  if (reason === "error") return "Failed";
  return "Needs attention";
}

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
  return attentionReasonLabel(attention.reason);
}
