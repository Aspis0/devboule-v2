import { loadStoredDesignHistory, updateStoredDesignHistory } from "./designSettings";
import type { Session } from "../../types/ipc";

export { DESIGN_SETTINGS_BYTE_BUDGET, MAX_SURFACE_SETTINGS_BYTES } from "./designSettings";

export const MAX_HISTORY_ENTRIES = 32;
export const MAX_HISTORY_TITLE_CHARS = 256;
export const MAX_HISTORY_SESSION_ID_CHARS = 64;
export const MAX_HISTORY_PEER_SESSION_ID_CHARS = 128;

export interface DesignHistoryEntry {
  sessionId: string;
  /** From Session.peerSessionId; a differing value identifies a reused session id's other run. */
  peerSessionId: string | null;
  /** From the producing Session; null preserves entries written before this field existed. */
  createdAtMs: number | null;
  /** The prompt that started the run, trimmed. */
  title: string;
  savedAtMs: number;
  /** Where the run was started from. Only "design" can occur today. */
  origin: "design" | "workspace";
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function trimCharacters(value: string, maximum: number): string {
  return Array.from(value).slice(0, maximum).join("");
}

function parseHistoryEntry(value: unknown): DesignHistoryEntry | null {
  if (!isRecord(value)) return null;
  const sessionId = value.sessionId;
  const peerSessionId = value.peerSessionId;
  const createdAtMs = value.createdAtMs;
  const title = value.title;
  const savedAtMs = value.savedAtMs;
  const origin = value.origin;
  if (
    typeof sessionId !== "string" ||
    sessionId.length === 0 ||
    Array.from(sessionId).length > MAX_HISTORY_SESSION_ID_CHARS ||
    (typeof peerSessionId !== "string" && peerSessionId !== null) ||
    (typeof peerSessionId === "string" &&
      (peerSessionId.length === 0 ||
        Array.from(peerSessionId).length > MAX_HISTORY_PEER_SESSION_ID_CHARS)) ||
    (createdAtMs !== undefined &&
      createdAtMs !== null &&
      (typeof createdAtMs !== "number" || !Number.isFinite(createdAtMs))) ||
    typeof title !== "string" ||
    typeof savedAtMs !== "number" ||
    !Number.isFinite(savedAtMs) ||
    (origin !== "design" && origin !== "workspace")
  ) {
    return null;
  }

  return {
    sessionId,
    peerSessionId,
    createdAtMs: typeof createdAtMs === "number" ? createdAtMs : null,
    title: trimCharacters(title.trim(), MAX_HISTORY_TITLE_CHARS),
    savedAtMs,
    origin,
  };
}

function newestFirst(entries: readonly DesignHistoryEntry[]): DesignHistoryEntry[] {
  return [...entries].sort((left, right) => {
    if (left.savedAtMs === right.savedAtMs) return 0;
    return left.savedAtMs > right.savedAtMs ? -1 : 1;
  });
}

function parseHistoryEntries(values: readonly unknown[]): DesignHistoryEntry[] {
  return values.flatMap((value) => {
    const entry = parseHistoryEntry(value);
    return entry === null ? [] : [entry];
  });
}

export async function loadDesignHistory(): Promise<DesignHistoryEntry[] | null> {
  try {
    const storedHistory = await loadStoredDesignHistory();
    if (storedHistory === null) return null;
    return newestFirst(parseHistoryEntries(storedHistory)).slice(0, MAX_HISTORY_ENTRIES);
  } catch {
    // A failed read is distinct from a successful empty history.
    return null;
  }
}

// The boolean reports whether the entry reached storage; a false return means the run was
// generated but its history entry was not saved, so the caller can surface that loss.
export async function recordDesignHistoryEntry(entry: DesignHistoryEntry): Promise<boolean> {
  const normalized = parseHistoryEntry(entry);
  // A malformed entry was never persisted, which is a reported outcome here, not silence.
  if (normalized === null) return false;

  try {
    await updateStoredDesignHistory((stored) => {
      const entries = parseHistoryEntries(stored).filter(
        (current) => current.sessionId !== normalized.sessionId,
      );
      entries.push(normalized);
      return newestFirst(entries).slice(0, MAX_HISTORY_ENTRIES);
    });
    return true;
  } catch {
    // History is a convenience index; a settings failure must not fail a generation.
    return false;
  }
}

export function historyEntryStatus(
  entry: DesignHistoryEntry,
  sessions: readonly Session[],
): "available" | "gone" {
  const session = sessions.find((candidate) => candidate.id === entry.sessionId);
  if (session === undefined) return "gone";

  if (typeof entry.createdAtMs === "number" && typeof session.createdAtMs === "number") {
    return entry.createdAtMs === session.createdAtMs ? "available" : "gone";
  }

  const currentPeerSessionId = session.peerSessionId;
  if (entry.peerSessionId === null) {
    // Entries recorded before peer ids were tracked fall back to the session id alone.
    return "available";
  }
  if (currentPeerSessionId !== undefined) {
    return entry.peerSessionId === currentPeerSessionId ? "available" : "gone";
  }

  // A missing session peer id cannot prove that this is the same reused session; a wrong
  // transcript is worse than saying a correct one is missing.
  return "gone";
}
