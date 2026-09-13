import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  surfaceSettingsGet: vi.fn(),
  surfaceSettingsSet: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  surfaceSettingsGet: mocks.surfaceSettingsGet,
  surfaceSettingsSet: mocks.surfaceSettingsSet,
}));

import type { SessionEvent } from "../../types/ipc";
import { childFinishedHistoryEntry, recordChildFinishedHistory } from "./childFinishedHistory";
import { loadDesignHistory, MAX_HISTORY_ENTRIES } from "./designHistory";
import { MAX_SURFACE_SETTINGS_BYTES } from "./designSettings";

type ChildFinished = Extract<SessionEvent, { type: "child_finished" }>;

let storedSettings: unknown = null;

const CHILD_FINISHED: ChildFinished = {
  type: "child_finished",
  messageId: "m2",
  childSessionId: "s.parent.2",
  displayName: "worker one",
  state: "completed",
  artifacts: [
    {
      artifactId: "a1",
      parts: [{ url: "devboule-artifact://s.parent.2/a1", mimeType: "text/markdown" }],
    },
  ],
};

beforeEach(() => {
  storedSettings = null;
  mocks.surfaceSettingsGet.mockReset();
  mocks.surfaceSettingsSet.mockReset();
  mocks.surfaceSettingsGet.mockImplementation(async () => {
    await Promise.resolve();
    return storedSettings === null
      ? { status: "absent" }
      : { status: "value", value: storedSettings };
  });
  mocks.surfaceSettingsSet.mockImplementation(async (_surfaceId: string, value: unknown) => {
    await Promise.resolve();
    storedSettings = value;
  });
});

describe("child_finished history entry", () => {
  it("points at the child session and copies no artifact", () => {
    // The Design history holds pointers: `designHistoryOpen` re-attaches to the
    // session and re-extracts the html from the journal. The event's artifact is
    // a fallback for a replay that comes back empty, so the entry must not carry
    // its url — and above all not its content.
    expect(childFinishedHistoryEntry(CHILD_FINISHED, 1_760_000_000_000)).toEqual({
      sessionId: "s.parent.2",
      peerSessionId: null,
      createdAtMs: null,
      title: "worker one",
      savedAtMs: 1_760_000_000_000,
      origin: "child",
    });
  });

  it("names an unnamed child by its short id instead of leaving the row blank", () => {
    expect(childFinishedHistoryEntry({ ...CHILD_FINISHED, displayName: "  " }, 1).title).toBe(
      "s.parent",
    );
  });

  it("writes the entry with no Design surface mounted", async () => {
    // Nothing is rendered here on purpose: the finish is handled in the shared
    // session event pipeline (`AgentSession.handleEvent`), which is the only
    // listener that survives the user looking at another surface.
    await expect(recordChildFinishedHistory(CHILD_FINISHED, () => 4_242)).resolves.toBe(true);

    await expect(loadDesignHistory()).resolves.toEqual([
      {
        sessionId: "s.parent.2",
        peerSessionId: null,
        createdAtMs: null,
        title: "worker one",
        savedAtMs: 4_242,
        origin: "child",
      },
    ]);
    // The whole stored blob stays a pointer: the artifact's url is not there
    // either, because there is no read-by-reference door to use it through.
    expect(JSON.stringify(storedSettings)).not.toContain("devboule-artifact://");
  });

  it("writes one entry when the same finish arrives twice, as a replay would", async () => {
    await recordChildFinishedHistory(CHILD_FINISHED, () => 10);
    await recordChildFinishedHistory(CHILD_FINISHED, () => 20);

    const history = await loadDesignHistory();
    expect(history).toHaveLength(1);
    expect(history?.[0]).toMatchObject({ sessionId: "s.parent.2", savedAtMs: 20 });
  });

  it("keeps the whole history inside the settings byte budget", async () => {
    for (let index = 0; index < MAX_HISTORY_ENTRIES + 4; index += 1) {
      await recordChildFinishedHistory(
        { ...CHILD_FINISHED, childSessionId: `s.parent.${index}` },
        () => index,
      );
    }

    const history = await loadDesignHistory();
    expect(history).toHaveLength(MAX_HISTORY_ENTRIES);
    expect(JSON.stringify(storedSettings).length).toBeLessThan(MAX_SURFACE_SETTINGS_BYTES);
  });

  it("reports a lost write instead of throwing into the event pipeline", async () => {
    mocks.surfaceSettingsSet.mockRejectedValue(new Error("surface settings are unwritable"));
    // A history entry the settings file would not take must not take the event
    // pipeline down with it: the boolean is the whole report.
    await expect(recordChildFinishedHistory(CHILD_FINISHED, () => 1)).resolves.toBe(false);
    await expect(loadDesignHistory()).resolves.toEqual([]);
  });
});
