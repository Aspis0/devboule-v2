/**
 * The stored "When I close the window" choice, shared by the Settings row
 * that writes it and the close flow that reads it back.
 *
 * The document lives under the `close-behavior` surface id as
 * `{ choice: "ask" | "tray" | "quit" }`. The Rust close flow
 * (`src-tauri/src/close_prompt.rs`) parses the same file with the same
 * rule: anything missing or unrecognizable means "ask" — a stored value
 * this build does not know must never skip the confirmation, because the
 * confirmation is what keeps a quit from stopping a daemon silently.
 */
export type CloseBehaviorChoice = "ask" | "tray" | "quit";

export const CLOSE_BEHAVIOR_SURFACE_ID = "close-behavior";

export function closeChoiceFromStored(value: unknown): CloseBehaviorChoice {
  if (typeof value === "object" && value !== null && "choice" in value) {
    const choice = (value as { choice: unknown }).choice;
    if (choice === "ask" || choice === "tray" || choice === "quit") return choice;
  }
  return "ask";
}
