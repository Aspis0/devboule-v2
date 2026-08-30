import { describe, expect, it } from "vitest";
import { describePluginState, pluginState, pluginTone, POLIS_PLUGIN_ID } from "./plugins";
import type { PluginEntry, PluginInventory } from "../types/ipc";

function inventory(plugins: PluginEntry[], problem: string | null = null): PluginInventory {
  return { root: "C:/data/plugins", plugins, problem };
}

const READY: PluginEntry = {
  id: POLIS_PLUGIN_ID,
  name: "Polis",
  version: "0.1.0",
  capabilities: ["oracle.search"],
  ready: true,
  reason: null,
};

const REFUSED: PluginEntry = {
  id: POLIS_PLUGIN_ID,
  name: null,
  version: null,
  capabilities: [],
  ready: false,
  reason: "ui/index.js is not the file plugin.json describes",
};

describe("pluginState", () => {
  it("reads a verified plugin as ready", () => {
    const state = pluginState(inventory([READY]), POLIS_PLUGIN_ID);
    expect(state.kind).toBe("ready");
    expect(describePluginState(state, POLIS_PLUGIN_ID)).toBe(
      "Polis 0.1.0 is installed and verified",
    );
    expect(pluginTone(state)).toBe("ready");
  });

  it("keeps the refusal reason instead of collapsing it to not installed", () => {
    const state = pluginState(inventory([REFUSED]), POLIS_PLUGIN_ID);
    expect(state.kind).toBe("refused");
    const line = describePluginState(state, POLIS_PLUGIN_ID);
    expect(line).toContain("was refused");
    expect(line).toContain("ui/index.js");
    expect(pluginTone(state)).toBe("blocked");
  });

  it("says nothing dramatic when no plugin is installed", () => {
    const state = pluginState(inventory([]), POLIS_PLUGIN_ID);
    expect(state.kind).toBe("absent");
    expect(describePluginState(state, POLIS_PLUGIN_ID)).toBe("polis is not installed");
    // Shipping without Polis is the product, not a fault.
    expect(pluginTone(state)).toBe("unknown");
  });

  it("does not report an unreadable directory as an empty one", () => {
    const state = pluginState(inventory([], "the drive is not mounted"), POLIS_PLUGIN_ID);
    expect(state.kind).toBe("unknown");
    expect(describePluginState(state, POLIS_PLUGIN_ID)).toContain("could not tell");
    expect(describePluginState(state, POLIS_PLUGIN_ID)).toContain("not mounted");
  });

  it("prefers what it found about the plugin over a partial-scan warning", () => {
    // The scan can list what it reached and still warn that the list may be
    // short. A plugin it did reach has a definite answer, and that answer is
    // more useful than the warning.
    const state = pluginState(inventory([READY], "the list may be short"), POLIS_PLUGIN_ID);
    expect(state.kind).toBe("ready");
  });

  it("falls back to the id when a verified plugin reports no name", () => {
    const nameless = { ...READY, name: null, version: null };
    const state = pluginState(inventory([nameless]), POLIS_PLUGIN_ID);
    expect(describePluginState(state, POLIS_PLUGIN_ID)).toBe("polis is installed and verified");
  });
});
