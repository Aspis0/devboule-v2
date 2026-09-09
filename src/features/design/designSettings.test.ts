import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  surfaceSettingsGet: vi.fn(),
  surfaceSettingsSet: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  surfaceSettingsGet: mocks.surfaceSettingsGet,
  surfaceSettingsSet: mocks.surfaceSettingsSet,
}));

import {
  DEFAULT_DESIGN_SKILL_SELECTION,
  DOCTRINE_SETTINGS_SURFACE_ID,
  loadDesignProviderId,
  loadDesignSkillSelection,
  loadDesignWorkspaceId,
  loadStoredDesignWorkspaceId,
  saveDesignProviderId,
  saveDesignSkillSelection,
  saveDesignWorkspaceId,
  SKILL_MODE_LABELS,
  selectedSlugs,
  updateStoredDesignHistory,
  type DesignSkillSelection,
} from "./designSettings";
import { MAX_AUTOMATIC_SKILL_SECTIONS } from "./builtInSkills";
import { recordDesignHistoryEntry, type DesignHistoryEntry } from "./designHistory";

const KNOWN_SLUGS = ["color", "motion", "spacing"] as const;

let storedSettings: unknown = null;

function expectDefaultFor(value: unknown): Promise<void> {
  // The mocked wrapper answers in the SurfaceSettingsRead shape the real one
  // guarantees: a null document is the backend's Ok(None), i.e. absent.
  mocks.surfaceSettingsGet.mockResolvedValueOnce(
    value === null ? { status: "absent" } : { status: "value", value },
  );
  return expect(loadDesignSkillSelection(KNOWN_SLUGS)).resolves.toEqual(
    DEFAULT_DESIGN_SKILL_SELECTION,
  );
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

describe("loadDesignSkillSelection", () => {
  it("keeps the visible mode names centralized", () => {
    expect(SKILL_MODE_LABELS).toMatchObject({
      all: {
        name: "Matched",
        summary: "request match · no extra turn",
        badge: "Default",
        defaultNotice:
          "Matched is used when you do not choose a mode; it selects relevant sections from your request without another model turn.",
        fallbackNotice: "No strong match — using the default order.",
      },
      manual: { name: "Manual", summary: "choose sections", badge: "You choose" },
      auto: {
        name: "Automatic",
        summary: "agent picks · +1 model turn",
        badge: "Extra model turn",
      },
    });
    // The fallback microcopy must be readable on its own, not embedded in the blurb.
    expect(SKILL_MODE_LABELS.all.blurb).not.toContain("No strong match");
    expect(SKILL_MODE_LABELS.all.blurb).toContain("no extra model turn");
  });

  it("falls back for null", async () => {
    await expectDefaultFor(null);
  });

  it("falls back for a primitive", async () => {
    await expectDefaultFor("settings");
  });

  it("falls back for an array", async () => {
    await expectDefaultFor([]);
  });

  it("falls back when version is missing", async () => {
    await expectDefaultFor({ mode: "all", enabledSlugs: [] });
  });

  it("falls back when version is not 1", async () => {
    await expectDefaultFor({ version: 2, mode: "all", enabledSlugs: [] });
  });

  it("falls back when mode is missing", async () => {
    await expectDefaultFor({ version: 1, enabledSlugs: [] });
  });

  it("falls back when mode is not supported", async () => {
    await expectDefaultFor({ version: 1, mode: "future", enabledSlugs: [] });
  });

  it("falls back when enabledSlugs is missing", async () => {
    await expectDefaultFor({ version: 1, mode: "manual" });
  });

  it("falls back when enabledSlugs is not an array", async () => {
    await expectDefaultFor({
      version: 1,
      mode: "manual",
      enabledSlugs: "color",
    });
  });

  it("falls back when enabledSlugs contains a non-string", async () => {
    await expectDefaultFor({
      version: 1,
      mode: "manual",
      enabledSlugs: [KNOWN_SLUGS[0], 1],
    });
  });

  it("falls back when reading settings rejects", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });

    await expect(loadDesignSkillSelection(KNOWN_SLUGS)).resolves.toEqual(
      DEFAULT_DESIGN_SKILL_SELECTION,
    );
  });

  it("drops stale slugs, collapses duplicates, and orders by known slugs", async () => {
    const storedSlugs = [KNOWN_SLUGS[2], "removed-skill", KNOWN_SLUGS[1], KNOWN_SLUGS[2]];
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "manual",
        enabledSlugs: storedSlugs,
      },
    });

    const result = await loadDesignSkillSelection(KNOWN_SLUGS);
    const expectedSlugs = KNOWN_SLUGS.filter(
      (slug, index) => storedSlugs.includes(slug) && KNOWN_SLUGS.indexOf(slug) === index,
    );

    expect(result).toEqual({
      version: 1,
      mode: "manual",
      enabledSlugs: expectedSlugs,
    });
  });

  it("clamps persisted manual selections to the derived safe maximum", async () => {
    const knownSlugs = ["one", "two", "three", "four", "five"];
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "manual",
        enabledSlugs: knownSlugs,
      },
    });

    await expect(loadDesignSkillSelection(knownSlugs)).resolves.toEqual({
      version: 1,
      mode: "manual",
      enabledSlugs: knownSlugs.slice(0, MAX_AUTOMATIC_SKILL_SECTIONS),
    });
  });

  it("preserves a manual list while all mode selects every known slug", async () => {
    const storedSlugs = [KNOWN_SLUGS[1], "removed-skill"];
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: storedSlugs,
      },
    });

    const result = await loadDesignSkillSelection(KNOWN_SLUGS);

    expect(result.mode).toBe("all");
    expect(result.enabledSlugs).toEqual([KNOWN_SLUGS[1]]);
    expect(selectedSlugs(result, KNOWN_SLUGS)).toEqual([...KNOWN_SLUGS]);
  });
});

describe("selectedSlugs", () => {
  it("returns every known slug in all mode", () => {
    const selection: DesignSkillSelection = {
      version: 1,
      mode: "all",
      enabledSlugs: [],
    };

    expect(selectedSlugs(selection, KNOWN_SLUGS)).toEqual([...KNOWN_SLUGS]);
  });

  it("returns the known intersection in known-slug order in manual mode", () => {
    const selection: DesignSkillSelection = {
      version: 1,
      mode: "manual",
      enabledSlugs: [KNOWN_SLUGS[2], KNOWN_SLUGS[0]],
    };

    expect(selectedSlugs(selection, KNOWN_SLUGS)).toEqual([KNOWN_SLUGS[0], KNOWN_SLUGS[2]]);
  });

  it("returns no doctrine for an empty manual selection", () => {
    const selection: DesignSkillSelection = {
      version: 1,
      mode: "manual",
      enabledSlugs: [],
    };

    expect(selectedSlugs(selection, KNOWN_SLUGS)).toEqual([]);
  });

  it("does not resolve automatic selection without request context", () => {
    const selection: DesignSkillSelection = {
      version: 1,
      mode: "auto",
      enabledSlugs: [KNOWN_SLUGS[0]],
    };

    expect(selectedSlugs(selection, KNOWN_SLUGS)).toEqual([]);
  });

  it("accepts automatic mode while preserving its manual list", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "auto",
        enabledSlugs: [KNOWN_SLUGS[1]],
      },
    });

    await expect(loadDesignSkillSelection(KNOWN_SLUGS)).resolves.toEqual({
      version: 1,
      mode: "auto",
      enabledSlugs: [KNOWN_SLUGS[1]],
    });
  });
});

describe("saveDesignSkillSelection", () => {
  it("writes the opaque selection to the design surface settings", async () => {
    const selection: DesignSkillSelection = {
      version: 1,
      mode: "manual",
      enabledSlugs: [KNOWN_SLUGS[1]],
    };

    await expect(saveDesignSkillSelection(selection)).resolves.toBe(true);

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, selection);
  });

  it("reports false when the settings write rejects", async () => {
    mocks.surfaceSettingsSet.mockRejectedValueOnce(new Error("settings unavailable"));

    await expect(saveDesignSkillSelection(DEFAULT_DESIGN_SKILL_SELECTION)).resolves.toBe(false);
  });

  it("reports false without writing when reading the existing document fails", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });

    await expect(saveDesignSkillSelection(DEFAULT_DESIGN_SKILL_SELECTION)).resolves.toBe(false);
    // A read-modify-write whose read failed must not replace the stored document with defaults.
    expect(mocks.surfaceSettingsSet).not.toHaveBeenCalled();
  });

  it("preserves the provider beside the doctrine selection", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [],
        providerId: "grok",
      },
    });
    const selection: DesignSkillSelection = {
      version: 1,
      mode: "manual",
      enabledSlugs: [KNOWN_SLUGS[0]],
    };

    await saveDesignSkillSelection(selection);

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, {
      ...selection,
      providerId: "grok",
    });
  });
});
describe("design provider settings", () => {
  it("resolves only a provider still present in the catalog", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      version: 1,
      mode: "all",
      enabledSlugs: [],
      providerId: "removed-agent",
    });

    await expect(loadDesignProviderId(["grok"])).resolves.toBeNull();
  });

  it("stores the provider and reports the save as persisted", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [KNOWN_SLUGS[0]],
      },
    });

    await expect(saveDesignProviderId("grok")).resolves.toBe(true);

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, {
      version: 1,
      mode: "all",
      enabledSlugs: [KNOWN_SLUGS[0]],
      providerId: "grok",
    });
  });

  it("reports false when the provider write rejects", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [],
      },
    });
    mocks.surfaceSettingsSet.mockRejectedValueOnce(new Error("settings unavailable"));

    await expect(saveDesignProviderId("grok")).resolves.toBe(false);
  });

  it("reports false without writing when reading the existing document fails", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });

    await expect(saveDesignProviderId("grok")).resolves.toBe(false);
    expect(mocks.surfaceSettingsSet).not.toHaveBeenCalled();
  });
});

describe("design workspace settings", () => {
  it("can read a stored workspace before an incomplete registry is validated", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [],
        workspaceId: "workspace-unconfirmed",
      },
    });

    await expect(loadStoredDesignWorkspaceId()).resolves.toBe("workspace-unconfirmed");
  });

  it("resolves only a workspace still present in the registry", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [],
        workspaceId: "removed-workspace",
      },
    });

    await expect(loadDesignWorkspaceId(["workspace-current"])).resolves.toBeNull();
  });

  it("stores the workspace beside the provider and reports the save as persisted", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [],
        providerId: "grok",
      },
    });

    await expect(saveDesignWorkspaceId("workspace-current")).resolves.toBe(true);

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, {
      version: 1,
      mode: "all",
      enabledSlugs: [],
      providerId: "grok",
      workspaceId: "workspace-current",
    });
  });

  it("reports false when the workspace write rejects", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [],
      },
    });
    mocks.surfaceSettingsSet.mockRejectedValueOnce(new Error("settings unavailable"));

    await expect(saveDesignWorkspaceId("workspace-current")).resolves.toBe(false);
  });

  it("reports false without writing when reading the existing document fails", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });

    await expect(saveDesignWorkspaceId("workspace-current")).resolves.toBe(false);
    expect(mocks.surfaceSettingsSet).not.toHaveBeenCalled();
  });

  it("removes a cleared workspace without erasing the provider", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "value",
      value: {
        version: 1,
        mode: "all",
        enabledSlugs: [],
        providerId: "grok",
        workspaceId: "workspace-old",
      },
    });

    await saveDesignWorkspaceId(null);

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, {
      version: 1,
      mode: "all",
      enabledSlugs: [],
      providerId: "grok",
    });
  });
});

describe("concurrent design settings writes", () => {
  const selection: DesignSkillSelection = {
    version: 1,
    mode: "manual",
    enabledSlugs: [KNOWN_SLUGS[0]],
  };

  it("preserves both fields when the selection write starts first", async () => {
    const selectionWrite = saveDesignSkillSelection(selection);
    const providerWrite = saveDesignProviderId("grok");

    await Promise.all([selectionWrite, providerWrite]);

    expect(storedSettings).toEqual({ ...selection, providerId: "grok" });
  });

  it("preserves both fields when the provider write starts first", async () => {
    const providerWrite = saveDesignProviderId("grok");
    const selectionWrite = saveDesignSkillSelection(selection);

    await Promise.all([providerWrite, selectionWrite]);

    expect(storedSettings).toEqual({ ...selection, providerId: "grok" });
  });

  it("preserves history when recording overlaps a selection write", async () => {
    const entry: DesignHistoryEntry = {
      sessionId: "session-history",
      peerSessionId: "peer-history",
      createdAtMs: null,
      title: "A generated design",
      savedAtMs: 10,
      origin: "design",
    };

    const historyWrite = recordDesignHistoryEntry(entry);
    const selectionWrite = saveDesignSkillSelection(selection);

    await Promise.all([historyWrite, selectionWrite]);

    expect(storedSettings).toEqual({ ...selection, history: [entry] });
  });
});

describe("read-modify-write safety", () => {
  const historyEntry: DesignHistoryEntry = {
    sessionId: "session-history",
    peerSessionId: "peer-history",
    createdAtMs: null,
    title: "A generated design",
    savedAtMs: 10,
    origin: "design",
  };

  it("rejects and never writes when reading the existing document fails", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });

    await expect(
      updateStoredDesignHistory((history) => [...history, historyEntry]),
    ).rejects.toThrow("Design settings could not be read; refusing to overwrite them.");
    expect(mocks.surfaceSettingsSet).not.toHaveBeenCalled();
  });

  it("writes defaults when the existing settings surface is genuinely empty", async () => {
    await expect(
      updateStoredDesignHistory((history) => [...history, historyEntry]),
    ).resolves.toBeUndefined();

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, {
      version: 1,
      mode: "all",
      enabledSlugs: [],
      history: [historyEntry],
    });
  });

  it("preserves every existing field when appending history", async () => {
    const existingEntry: DesignHistoryEntry = {
      ...historyEntry,
      sessionId: "existing-session",
      title: "Existing design",
    };
    storedSettings = {
      version: 1,
      mode: "manual",
      enabledSlugs: [KNOWN_SLUGS[0]],
      providerId: "grok",
      workspaceId: "workspace-current",
      history: [existingEntry],
    };

    await updateStoredDesignHistory((history) => [...history, historyEntry]);

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, {
      version: 1,
      mode: "manual",
      enabledSlugs: [KNOWN_SLUGS[0]],
      providerId: "grok",
      workspaceId: "workspace-current",
      history: [existingEntry, historyEntry],
    });
  });

  it("keeps the write queue usable after a failed read", async () => {
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      status: "unreadable",
      message: "settings unavailable",
    });
    const failedWrite = updateStoredDesignHistory((history) => [...history, historyEntry]);

    await expect(failedWrite).rejects.toThrow("Design settings could not be read");

    await expect(
      updateStoredDesignHistory((history) => [...history, historyEntry]),
    ).resolves.toBeUndefined();
    expect(mocks.surfaceSettingsSet).toHaveBeenCalledTimes(1);
  });
});
