import type { CommandError } from "../types/ipc";

/**
 * Tauri v2 rejects `invoke` with the JSON value of a `Serialize` error type
 * directly — not wrapped in `Error`, not a string. `CommandError` arrives as
 * `{ code, message, details? }`, which `String(cause)` would render as
 * "[object Object]".
 */
export function isCommandError(error: unknown): error is CommandError {
  if (typeof error !== "object" || error === null || Array.isArray(error)) return false;
  if (!("code" in error) || !("message" in error)) return false;
  return typeof error.code === "string" && typeof error.message === "string";
}
