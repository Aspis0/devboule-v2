/**
 * The `type` of an event this build has no case for, for the message a
 * controller's `never` guard raises. One readable word in the transcript, not
 * the whole payload — and one copy of it, because the agent session and the
 * terminal session raise the same sentence about their own event union.
 */
export function eventTypeName(event: unknown): string {
  if (typeof event !== "object" || event === null || !("type" in event)) return "unknown";
  const type = event.type;
  return typeof type === "string" && type.trim() ? type : "unknown";
}
