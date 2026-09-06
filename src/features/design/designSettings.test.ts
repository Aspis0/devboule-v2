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
  loadDesignSkillSelection,
  saveDesignSkillSelection,
  selectedSlugs,
  type DesignSkillSelection,
} from "./designSettings";

const KNOWN_SLUGS = ["color", "motion", "spacing"] as const;

function expectDefaultFor(value: unknown): Promise<void> {
  mocks.surfaceSettingsGet.mockResolvedValueOnce(value);
  return expect(loadDesignSkillSelection(KNOWN_SLUGS)).resolves.toEqual(
    DEFAULT_DESIGN_SKILL_SELECTION,
  );
}

beforeEach(() => {
  mocks.surfaceSettingsGet.mockReset();
  mocks.surfaceSettingsSet.mockReset();
  mocks.surfaceSettingsSet.mockResolvedValue(undefined);
});

describe("loadDesignSkillSelection", () => {
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
    await expectDefaultFor({ version: 1, mode: "auto", enabledSlugs: [] });
  });

  it("falls back when enabledSlugs is missing", async () => {
    await expectDefaultFor({ version: 1, mode: "manual" });
  });

  it("falls back when enabledSlugs is not an array", async () => {
    await expectDefaultFor({ version: 1, mode: "manual", enabledSlugs: "color" });
  });

  it("falls back when enabledSlugs contains a non-string", async () => {
    await expectDefaultFor({ version: 1, mode: "manual", enabledSlugs: [KNOWN_SLUGS[0], 1] });
  });

  it("falls back when reading settings rejects", async () => {
    mocks.surfaceSettingsGet.mockRejectedValueOnce(new Error("settings unavailable"));

    await expect(loadDesignSkillSelection(KNOWN_SLUGS)).resolves.toEqual(
      DEFAULT_DESIGN_SKILL_SELECTION,
    );
  });

  it("drops stale slugs, collapses duplicates, and orders by known slugs", async () => {
    const storedSlugs = [KNOWN_SLUGS[2], "removed-skill", KNOWN_SLUGS[1], KNOWN_SLUGS[2]];
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      version: 1,
      mode: "manual",
      enabledSlugs: storedSlugs,
    });

    const result = await loadDesignSkillSelection(KNOWN_SLUGS);
    const expectedSlugs = KNOWN_SLUGS.filter(
      (slug, index) => storedSlugs.includes(slug) && KNOWN_SLUGS.indexOf(slug) === index,
    );

    expect(result).toEqual({ version: 1, mode: "manual", enabledSlugs: expectedSlugs });
  });

  it("preserves a manual list while all mode selects every known slug", async () => {
    const storedSlugs = [KNOWN_SLUGS[1], "removed-skill"];
    mocks.surfaceSettingsGet.mockResolvedValueOnce({
      version: 1,
      mode: "all",
      enabledSlugs: storedSlugs,
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
});

describe("saveDesignSkillSelection", () => {
  it("writes the opaque selection to the design surface settings", async () => {
    const selection: DesignSkillSelection = {
      version: 1,
      mode: "manual",
      enabledSlugs: [KNOWN_SLUGS[1]],
    };

    await expect(saveDesignSkillSelection(selection)).resolves.toBeUndefined();

    expect(mocks.surfaceSettingsSet).toHaveBeenCalledWith(DOCTRINE_SETTINGS_SURFACE_ID, selection);
  });

  it("swallows a rejecting settings write", async () => {
    mocks.surfaceSettingsSet.mockRejectedValueOnce(new Error("settings unavailable"));

    await expect(saveDesignSkillSelection(DEFAULT_DESIGN_SKILL_SELECTION)).resolves.toBeUndefined();
  });
});
