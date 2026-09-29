// The tool content cache: keyed by workspace and path, forgotten with the tab.

import { describe, expect, it } from "vitest";
import { createToolContentCache, evictToolContent, toolContentKey } from "./toolContentCache";

describe("toolContentKey", () => {
  it("keys workspace and path together", () => {
    expect(toolContentKey("ws", "a.ts")).toBe("ws\na.ts");
    expect(toolContentKey("ws", "a.ts")).not.toBe(toolContentKey("ws", "b.ts"));
  });
});

describe("evictToolContent", () => {
  it("drops the tab's entries from every map", () => {
    const cache = createToolContentCache();
    cache.diffs.set(toolContentKey("ws", "a.ts"), { reply: null, failure: null });
    cache.fileCells.set(toolContentKey("ws", "a.ts"), {
      reply: null,
      staged: null,
      failure: null,
    });
    cache.fileCells.set(toolContentKey("ws", "b.ts"), {
      reply: null,
      staged: null,
      failure: null,
    });
    evictToolContent(cache, "ws", "a.ts");
    expect(cache.diffs.has(toolContentKey("ws", "a.ts"))).toBe(false);
    expect(cache.fileCells.has(toolContentKey("ws", "a.ts"))).toBe(false);
    expect(cache.fileCells.has(toolContentKey("ws", "b.ts"))).toBe(true);
  });
});
