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
  historyEntryInstruction,
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
  createdAtMs: 1_000,
  title: "First pass",
  savedAtMs: 100,
  origin: "design",
};

function session(id: string, peerSessionId?: string, createdAtMs?: number): Session {
  return {
    id,
    workspaceId: null,
    kind: "acp",
    title: "Agent",
    ...(peerSessionId === undefined ? {} : { peerSessionId }),
    ...(createdAtMs === undefined ? {} : { createdAtMs }),
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
    // Emulate the real wrapper's SurfaceSettingsRead result: null storage is
    // the backend's Ok(None), i.e. an absent surface file.
    return storedSettings === null
      ? { status: "absent" }
      : { status: "value", value: storedSettings };
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

  it("distinguishes a failed history read from a successful empty read", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });
    await expect(loadDesignHistory()).resolves.toBeNull();

    storedSettings = null;
    await expect(loadDesignHistory()).resolves.toEqual([]);
  });

  it("replaces the previous result for the same session and trims the title", async () => {
    await expect(recordDesignHistoryEntry(BASE_ENTRY)).resolves.toBe(true);
    await expect(
      recordDesignHistoryEntry({
        ...BASE_ENTRY,
        title: `  ${"x".repeat(MAX_HISTORY_TITLE_CHARS + 10)}  `,
        savedAtMs: 200,
      }),
    ).resolves.toBe(true);

    await expect(loadDesignHistory()).resolves.toEqual([
      {
        ...BASE_ENTRY,
        title: "x".repeat(MAX_HISTORY_TITLE_CHARS),
        // The title is replaced and the time is not: an entry already present
        // keeps the time it was first recorded at, so a replayed write cannot
        // re-date a run and re-sort the list around it.
        savedAtMs: 100,
      },
    ]);
  });

  it("keeps the first saved time of an entry a later record re-dates", async () => {
    // The bug this closes: `savedAtMs` is the clock of whoever writes, and on the
    // attach path that is the clock of the replay — so reopening a creator
    // re-dated all of its finished children, re-sorted the list and pushed the
    // user's own runs past the 32-entry cut.
    await expect(recordDesignHistoryEntry(BASE_ENTRY)).resolves.toBe(true);
    await expect(
      recordDesignHistoryEntry({ ...BASE_ENTRY, savedAtMs: BASE_ENTRY.savedAtMs + 5_000 }),
    ).resolves.toBe(true);

    await expect(loadDesignHistory()).resolves.toEqual([BASE_ENTRY]);
  });

  it("keeps only the newest entries at the cap", async () => {
    for (let index = 0; index < MAX_HISTORY_ENTRIES + 3; index += 1) {
      await expect(
        recordDesignHistoryEntry({
          ...BASE_ENTRY,
          sessionId: `session-${index}`,
          savedAtMs: index,
        }),
      ).resolves.toBe(true);
    }

    const history = await loadDesignHistory();
    if (history === null) throw new Error("History read unexpectedly failed");
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

  it("reports whether a history entry was persisted", async () => {
    await expect(recordDesignHistoryEntry(BASE_ENTRY)).resolves.toBe(true);

    mocks.surfaceSettingsSet.mockRejectedValueOnce(new Error("settings unavailable"));
    // A second, different entry: re-recording BASE_ENTRY writes nothing at all now
    // (an unchanged document is not written), so a rejection could not be seen.
    await expect(
      recordDesignHistoryEntry({ ...BASE_ENTRY, sessionId: "session-2", savedAtMs: 200 }),
    ).resolves.toBe(false);
  });

  it("reports false without writing when reading the existing document fails", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });

    await expect(recordDesignHistoryEntry(BASE_ENTRY)).resolves.toBe(false);
    // A failed read cannot safely supply the document the append must preserve.
    expect(mocks.surfaceSettingsSet).not.toHaveBeenCalled();
  });

  it("reports false for a malformed entry without touching storage", async () => {
    await expect(recordDesignHistoryEntry({ ...BASE_ENTRY, sessionId: "" })).resolves.toBe(false);
    expect(mocks.surfaceSettingsSet).not.toHaveBeenCalled();
  });

  it("returns gone for a missing session and for a changed peer session", () => {
    expect(historyEntryStatus(BASE_ENTRY, [])).toBe("gone");
    expect(historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId, "peer-2")])).toBe("gone");
    expect(historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId, "peer-1")])).toBe(
      "available",
    );
  });

  it("uses matching creation times before peer ids", () => {
    expect(
      historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId, "peer-reissued", 1_000)]),
    ).toBe("available");
  });

  it("returns gone for differing creation times before peer ids", () => {
    expect(historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId, "peer-1", 2_000)])).toBe(
      "gone",
    );
  });

  it("falls back to peer ids when either creation time is absent", () => {
    const missingEntryTime = { ...BASE_ENTRY, createdAtMs: null };
    expect(
      historyEntryStatus(missingEntryTime, [session(BASE_ENTRY.sessionId, "peer-1", 2_000)]),
    ).toBe("available");
    expect(historyEntryStatus(BASE_ENTRY, [session(BASE_ENTRY.sessionId, "peer-1")])).toBe(
      "available",
    );
  });

  it("keeps legacy entries without a creation time on the peer-id fallback", async () => {
    const legacyEntry = {
      sessionId: BASE_ENTRY.sessionId,
      peerSessionId: BASE_ENTRY.peerSessionId,
      title: BASE_ENTRY.title,
      savedAtMs: BASE_ENTRY.savedAtMs,
      origin: BASE_ENTRY.origin,
    };
    storedSettings = {
      version: 1,
      mode: "all",
      enabledSlugs: [],
      history: [legacyEntry],
    };

    const history = await loadDesignHistory();
    if (history === null) throw new Error("History read unexpectedly failed");
    const [loaded] = history;
    expect(loaded).toEqual({ ...BASE_ENTRY, createdAtMs: null });
    expect(
      loaded === undefined
        ? "gone"
        : historyEntryStatus(loaded, [session(BASE_ENTRY.sessionId, "peer-1", 2_000)]),
    ).toBe("available");
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

describe("design history entry origin", () => {
  it("round-trips a commissioned child's entry through the parser", async () => {
    const childEntry: DesignHistoryEntry = {
      sessionId: "s.parent.2",
      peerSessionId: null,
      createdAtMs: null,
      title: "worker one",
      savedAtMs: 4_242,
      origin: "child",
    };

    await expect(recordDesignHistoryEntry(childEntry)).resolves.toBe(true);

    const history = await loadDesignHistory();
    expect(history).toEqual([childEntry]);
    // The widened union is what the parser admits, not a new spelling: the two
    // origins that existed before still survive the same read.
    expect(history?.[0]?.origin).toBe("child");
  });

  it("still reads every entry an older build wrote", async () => {
    // The older builds wrote `design` (the surface's own runs) and `workspace`
    // (reserved, never written by that name yet). Both must parse unchanged, and
    // so must a record with no `createdAtMs` at all.
    const olderDesign = {
      sessionId: "session-old",
      peerSessionId: "peer-old",
      title: "First pass",
      savedAtMs: 10,
      origin: "design",
    };
    const olderWorkspace = { ...olderDesign, sessionId: "session-ws", origin: "workspace" };
    storedSettings = {
      version: 1,
      mode: "all",
      enabledSlugs: [],
      history: [olderDesign, olderWorkspace],
    };

    await expect(loadDesignHistory()).resolves.toEqual([
      { ...olderDesign, createdAtMs: null },
      { ...olderWorkspace, createdAtMs: null },
    ]);
  });

  it("drops an entry whose origin no build ever wrote", async () => {
    storedSettings = {
      version: 1,
      mode: "all",
      enabledSlugs: [],
      history: [{ ...BASE_ENTRY, origin: "kiosk" }],
    };

    await expect(loadDesignHistory()).resolves.toEqual([]);
  });

  it("offers a reopen the prompt of its own runs and no prompt for a child's pointer", () => {
    // The bug this closes: a child entry's title is the commissioned agent's
    // display name, and the reopen handed it over as an instruction — so the
    // card's only action generated a design from the prompt "worker one".
    expect(historyEntryInstruction(BASE_ENTRY)).toBe("First pass");
    expect(
      historyEntryInstruction({ ...BASE_ENTRY, origin: "child", title: "worker one" }),
    ).toBeNull();
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
        createdAtMs: Number.MAX_SAFE_INTEGER - index,
        title: pathologicalTitle,
        savedAtMs: Number.MAX_SAFE_INTEGER - index,
        origin: "workspace",
      }),
    );
    storedSettings = {
      version: 1,
      mode: "manual",
      enabledSlugs: [...builtInSkillSlugs()],
      workspaceId: "workspace-history",
      history: maximumEntries,
    };
    const newestEntry: DesignHistoryEntry = {
      sessionId: "newest-session",
      peerSessionId: pathologicalPeerSessionId,
      createdAtMs: Number.MAX_SAFE_INTEGER + 1,
      title: pathologicalTitle,
      savedAtMs: Number.MAX_SAFE_INTEGER + 1,
      origin: "design",
    };

    await expect(recordDesignHistoryEntry(newestEntry)).resolves.toBe(true);

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
