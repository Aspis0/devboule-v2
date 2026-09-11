import { isCommandError } from "./tauri";

export function formatCount(value: number): string {
  return value.toLocaleString("en-US").replaceAll(",", " ");
}

export function commandErrorMessage(error: unknown): string {
  if (isCommandError(error) && error.message.trim()) return error.message;
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "Unknown Oracle error.";
}
