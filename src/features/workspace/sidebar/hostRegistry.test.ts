import { describe, expect, it } from "vitest";
import {
  HOST_REGISTRY_STORAGE_KEY,
  hostOrder,
  readHostRegistry,
  withDiscoveredHosts,
  withHostCollapsed,
  writeHostRegistry,
} from "./hostRegistry";

/** A store held in memory, so a write can be read back by a later reader. */
function memoryStore(seed: Record<string, string> = {}) {
  const entries = new Map(Object.entries(seed));
  return {
    getItem: (key: string) => entries.get(key) ?? null,
    setItem: (key: string, value: string) => {
      entries.set(key, value);
    },
    raw: (key: string) => entries.get(key) ?? null,
  };
}

function refusingStore() {
  return {
    getItem: () => null,
    setItem: () => {
      throw new Error("QuotaExceededError");
    },
  };
}

describe("the host registry the sidebar remembers", () => {
  it("is empty when no store has ever heard of one", () => {
    expect(readHostRegistry(memoryStore())).toEqual({});
    expect(readHostRegistry(null)).toEqual({});
  });

  it("keeps a fold and a place across a restart", () => {
    const store = memoryStore();

    let registry = withDiscoveredHosts(readHostRegistry(store), ["local", "dev-studio"]);
    registry = withHostCollapsed(registry, "dev-studio", true);
    writeHostRegistry(store, registry);

    const afterRestart = readHostRegistry(store);
    expect(afterRestart).toEqual(registry);
    expect(hostOrder(afterRestart)).toEqual(["local", "dev-studio"]);
    expect(afterRestart["dev-studio"]?.collapsed).toBe(true);
  });

  it("reads a record of another version as no registry at all", () => {
    const store = memoryStore({
      [HOST_REGISTRY_STORAGE_KEY]: JSON.stringify({
        v: 99,
        hosts: { "dev-studio": { order: 0, collapsed: true } },
      }),
    });

    expect(readHostRegistry(store)).toEqual({});
  });

  it("reads unparseable text as no registry at all", () => {
    const store = memoryStore({ [HOST_REGISTRY_STORAGE_KEY]: "{ not json" });

    expect(readHostRegistry(store)).toEqual({});
  });

  it("keeps the hosts it can read and drops the entries it cannot", () => {
    const store = memoryStore({
      [HOST_REGISTRY_STORAGE_KEY]: JSON.stringify({
        v: 1,
        hosts: {
          "dev-a": { order: 0 },
          "dev-b": "nonsense",
          "dev-c": { order: "first" },
          "dev-d": { order: 2, collapsed: "yes" },
        },
      }),
    });

    expect(readHostRegistry(store)).toEqual({
      "dev-a": { order: 0, collapsed: false },
      "dev-d": { order: 1, collapsed: false },
    });
  });

  it("loses the record, not the fold, when the store refuses the write", () => {
    const registry = withHostCollapsed(
      withDiscoveredHosts(readHostRegistry(null), ["dev-a"]),
      "dev-a",
      true,
    );

    expect(() => writeHostRegistry(refusingStore(), registry)).not.toThrow();
  });

  it("puts a host it has never met at the end, and appends it once", () => {
    const first = withDiscoveredHosts(readHostRegistry(null), ["local", "dev-a"]);
    const second = withDiscoveredHosts(first, ["local", "dev-a", "dev-b"]);

    expect(hostOrder(second)).toEqual(["local", "dev-a", "dev-b"]);
    // The same answer repeated hands back the very same record, so a caller
    // that writes on a change has nothing to write.
    expect(withDiscoveredHosts(second, ["local", "dev-a", "dev-b"])).toBe(second);
  });

  it("keeps a host's place while it is away from the device list", () => {
    const registry = withDiscoveredHosts(readHostRegistry(null), ["local", "dev-a", "dev-b"]);

    const whileAway = withDiscoveredHosts(registry, ["local", "dev-b"]);

    expect(hostOrder(whileAway)).toEqual(["local", "dev-a", "dev-b"]);
    // And it comes back to that place rather than being appended afresh.
    expect(withDiscoveredHosts(whileAway, ["local", "dev-a", "dev-b"])).toBe(whileAway);
  });

  it("reads a fold as a flag, not as another entry", () => {
    const open = withDiscoveredHosts(readHostRegistry(null), ["dev-a"]);

    const folded = withHostCollapsed(open, "dev-a", true);

    expect(hostOrder(folded)).toEqual(["dev-a"]);
    expect(withHostCollapsed(folded, "dev-a", false)).toEqual(open);
  });
});

describe("a record whose positions cannot be trusted", () => {
  function stored(hosts: unknown) {
    return readHostRegistry(
      memoryStore({ [HOST_REGISTRY_STORAGE_KEY]: JSON.stringify({ v: 1, hosts }) }),
    );
  }

  function positions(registry: ReturnType<typeof stored>) {
    return Object.values(registry).map((entry) => entry.order);
  }

  it("loads negative, fractional and huge positions into a clean run", () => {
    const registry = stored({
      "dev-huge": { order: 1e300, collapsed: false },
      "dev-real": { order: 4, collapsed: false },
      "dev-negative": { order: -3, collapsed: false },
      "dev-fraction": { order: 1.5, collapsed: false },
    });

    // Only the position we can trust is believed; the rest follow it by id.
    expect(hostOrder(registry)).toEqual(["dev-real", "dev-fraction", "dev-huge", "dev-negative"]);
    expect(positions(registry)).toEqual([0, 1, 2, 3]);
  });

  it("settles two hosts that claim the same position by their ids", () => {
    const registry = stored({
      "dev-b": { order: 0, collapsed: false },
      "dev-a": { order: 0, collapsed: false },
    });

    expect(hostOrder(registry)).toEqual(["dev-a", "dev-b"]);
    expect(positions(registry)).toEqual([0, 1]);
  });

  it("appends a new host past every position it loaded", () => {
    const grown = withDiscoveredHosts(
      stored({
        "dev-b": { order: -1, collapsed: false },
        "dev-a": { order: 9, collapsed: false },
      }),
      ["dev-a", "dev-b", "dev-new"],
    );

    expect(hostOrder(grown)).toEqual(["dev-a", "dev-b", "dev-new"]);
    expect(positions(grown)).toEqual([0, 1, 2]);
  });

  it("keeps the fold of a host whose position is lost", () => {
    const registry = stored({
      "dev-far": { order: -1, collapsed: true },
      "dev-near": { order: 2, collapsed: false },
    });

    expect(registry["dev-far"]).toEqual({ order: 1, collapsed: true });
  });

  it("drops an entry with nothing to keep", () => {
    const registry = stored({
      "dev-no-flag": { order: -1 },
      "dev-wrong-flag": { order: -1, collapsed: "yes" },
      "dev-not-a-record": "nonsense",
      "dev-trustworthy": { order: 0 },
    });

    expect(Object.keys(registry)).toEqual(["dev-trustworthy"]);
    expect(registry["dev-trustworthy"]).toEqual({ order: 0, collapsed: false });
  });
});
