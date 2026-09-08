import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  surfaceSettingsGet: vi.fn(),
  surfaceSettingsSet: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  surfaceSettingsGet: mocks.surfaceSettingsGet,
  surfaceSettingsSet: mocks.surfaceSettingsSet,
}));

import { builtInSkillSlugs } from "./builtInSkills";
import {
  historyEntryStatus,
  loadDesignHistory,
  MAX_HISTORY_ENTRIES,
  MAX_HISTORY_PEER_SESSION_ID_CHARS,
  MAX_HISTORY_SESSION_ID_CHARS,
  MAX_HISTORY_TITLE_CHARS,
  MAX_SURFACE_SETTINGS_BYTES,
  recordDesignHistoryEntry,
  type DesignHistoryEntry,
} from "./designHistory";
import { loadDesignSkillSelection } from "./designSettings";
import type { Session } from "../../types/ipc";

let storedSettings: unknown = null;

const BASE_ENTRY: DesignHistoryEntry = {
  sessionId: "session-1",
  peerSessionId: "peer-1",
  title: "First pass",
  savedAtMs: 100,
  origin: "design",
};

function session(id: string, peerSessionId?: string): Session {
  return {
    id,
    workspaceId: null,
    kind: "acp",
    title: "Agent",
    ...(peerSessionId === undefined ? {} : { peerSessionId }),
    state: { type: "ended", generation: 1, code: 0, integrity: { kind: "complete" } },
    elapsedMs: null,
  };
}

beforeEach(() => {
  storedSettings = null;
  mocks.surfaceSettingsGet.mockReset();
  mocks.surfaceSettingsSet.mockReset();
  mocks.surfaceSettingsGet.mockImplementation(async () => {
    await Promise.resolve();
    return storedSettings;
  });
  mocks.surfaceSettingsSet.mockImplementation(async (_surfaceId: string, value: unknown) => {
    await Promise.resolve();
    storedSettings = value;
  });
});

describe("design history persistence", () => {
  it("loads entries newest first", async () => {
    storedSettings = {
      version: 1,
      mode: "all",
      enabledSlugs: [],
      history: [
        { ...BASE_ENTRY, sessionId: "older", savedAtMs: 10 },
        { ...BASE_ENTRY, sessionId: "newer", savedAtMs: 20 },
      ],
    };

    await expect(loadDesignHistory()).resolves.toMatchObject([
      { sessionId: "newer" },
      { sessionId: "older" },
    ]);
  });

  it("replaces the previous result for the same session and trims the title", async () => {
    await recordDesignHistoryEntry(BASE_ENTRY);
    await recordDesignHistoryEntry({
      ...BASE_ENTRY,
      title: `  ${"x".repeat(MAX_HISTORY_TITLE_CHARS + 10)}  `,
      savedAtMs: 200,
    });

    await expect(loadDesignHistory()).resolves.toEqual([
      {
        ...BASE_ENTRY,
        title: "x".repeat(MAX_HISTORY_TITLE_CHARS),
        savedAtMs: 200,
      },
    ]);
  });

  it("keeps only the newest entries at the cap", async () => {
    for (let index = 0; index < MAX_HISTORY_ENTRIES + 3; index += 1) {
      await recordDesignHistoryEntry({
        ...BASE_ENTRY,
        sessionId: `session-${index}`,
        savedAtMs: index,
      });
    }

    const history = await loadDesignHistory();
    expect(history).toHaveLength(MAX_HISTORY_ENTRIES);
    expect(history[0]?.sessionId).toBe(`session-${MAX_HISTORY_ENTRIES + 2}`);
    expect(history.at(-1)?.sessionId).toBe("session-3");
  });

  it("drops malformed entries without losing the doctrine selection", async () => {
    storedSettings = {
      version: 1,
      mode: "manual",
      enabledSlugs: ["spacing"],
      history: [BASE_ENTRY, { sessionId: "missing-title", savedAtMs: 3 }],
    };

    await expect(loadDesignHistory()).resolves.toEqual([BASE_ENTRY]);
    await expect(loadDesignSkillSelection(["spacing", "color"])).resolves.toEqual({
      version: 1,
      mode: "manual",
      enabledSlugs: ["spacing"],
    });
  });

  it("keeps history when the doctrine shape is corrupt", async () => {
    storedSettings = {
      version: "corrupt",
      mode: "not-a-mode",
      enabledSlugs: "not-an-array",
      history: [BASE_ENTRY],
    };

    await expect(loadDesignHistory()).resolves.toEqual([BASE_ENTRY]);
    await expect(loadDesignSkillSelection(["spacing", "color"])).resolves.toEqual({
      version: 1,
      mode: "all",
      enabledSlugs: [],
    });
  });

  it("keeps the doctrine selection when the history shape is corrupt", async () => {
    storedSettings = {
      version: 1,
      mode: "manual",
      enabledSlugs: ["spacing"],
      history: "not-an-array",
    };

    await expect(loadDesignHistory()).resolves.toEqual([]);
    await expect(loadDesignSkillSelection(["spacing", "color"])).resolves.toEqual({
      version: 1,
      mode: "manual",
      enabledSlugs: ["spacing"],
    });
  });

  it("returns gone for a missing session and for a changed peer session", () => {
    expect(historyEntryStatus(BASE_ENTRY, [])).toBe("gone");
    expect(historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId, "peer-2")])).toBe("gone");
    expect(historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId, "peer-1")])).toBe(
      "available",
    );
  });

  it("returns gone when the entry has a peer id but the session does not", () => {
    expect(historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId)])).toBe("gone");
  });

  it("falls back to the session id for entries without a peer id", () => {
    const legacyEntry = { ...BASE_ENTRY, peerSessionId: null };
    expect(historyEntryStatus(legacyEntry, [session(legacyEntry.sessionId, "peer-now")])).toBe(
      "available",
    );
  });
});

describe("design history settings size", () => {
  it("trims pathological history by measured bytes and keeps the newest entry", async () => {
    const pathologicalString = (length: number): string =>
      Array.from({ length }, (_, index) => (index % 2 === 0 ? "\u0000" : "😀")).join("");
    const pathologicalTitle = pathologicalString(MAX_HISTORY_TITLE_CHARS);
    const pathologicalPeerSessionId = pathologicalString(MAX_HISTORY_PEER_SESSION_ID_CHARS);
    const maximumEntries: DesignHistoryEntry[] = Array.from(
      { length: MAX_HISTORY_ENTRIES },
      (_, index) => ({
        sessionId: `${String(index).padStart(2, "0")}${"s".repeat(MAX_HISTORY_SESSION_ID_CHARS - 2)}`,
        peerSessionId: pathologicalPeerSessionId,
        title: pathologicalTitle,
        savedAtMs: Number.MAX_SAFE_INTEGER - index,
        origin: "workspace",
      }),
    );
    storedSettings = {
      version: 1,
      mode: "manual",
      enabledSlugs: [...builtInSkillSlugs()],
      history: maximumEntries,
    };
    const newestEntry: DesignHistoryEntry = {
      sessionId: "newest-session",
      peerSessionId: pathologicalPeerSessionId,
      title: pathologicalTitle,
      savedAtMs: Number.MAX_SAFE_INTEGER + 1,
      origin: "design",
    };

    await expect(recordDesignHistoryEntry(newestEntry)).resolves.toBeUndefined();

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledTimes(1);
    expect(storedSettings).toMatchObject({
      version: 1,
      mode: "manual",
      enabledSlugs: [...builtInSkillSlugs()],
    });
    const savedDocument = storedSettings as { history?: unknown };
    const savedHistory = savedDocument.history;
    expect(Array.isArray(savedHistory)).toBe(true);
    expect(savedHistory).toEqual(expect.arrayContaining([newestEntry]));
    expect(savedHistory).not.toEqual(expect.arrayContaining([maximumEntries.at(-1)]));
    const bytes = new TextEncoder().encode(JSON.stringify(storedSettings)).byteLength;
    expect(bytes).toBeLessThanOrEqual(MAX_SURFACE_SETTINGS_BYTES);
  });
});
