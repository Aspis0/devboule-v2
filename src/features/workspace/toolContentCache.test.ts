// The tool content cache: keyed by workspace and path, forgotten with the tab.

import { describe, expect, it } from "vitest";
import { createToolContentCache, evictToolContent, toolContentKey } from "./toolContentCache";
import { localWorkspaceKey, type WorkspaceKey } from "./hosts/hostIdentity";

const keyFor = (workspaceId: string): WorkspaceKey => localWorkspaceKey(workspaceId)!;

describe("toolContentKey", () => {
  it("keys workspace and path together", () => {
    expect(toolContentKey(keyFor("ws"), "a.ts")).toBe("local:ws\na.ts");
    expect(toolContentKey(keyFor("ws"), "a.ts")).not.toBe(toolContentKey(keyFor("ws"), "b.ts"));
  });
});

describe("evictToolContent", () => {
  it("drops the tab's entries from every map", () => {
    const cache = createToolContentCache();
    cache.diffs.set(toolContentKey(keyFor("ws"), "a.ts"), { reply: null, failure: null });
    cache.fileCells.set(toolContentKey(keyFor("ws"), "a.ts"), {
      reply: null,
      staged: null,
      failure: null,
    });
    cache.fileCells.set(toolContentKey(keyFor("ws"), "b.ts"), {
      reply: null,
      staged: null,
      failure: null,
    });
    evictToolContent(cache, keyFor("ws"), "a.ts");
    expect(cache.diffs.has(toolContentKey(keyFor("ws"), "a.ts"))).toBe(false);
    expect(cache.fileCells.has(toolContentKey(keyFor("ws"), "a.ts"))).toBe(false);
    expect(cache.fileCells.has(toolContentKey(keyFor("ws"), "b.ts"))).toBe(true);
  });
});
