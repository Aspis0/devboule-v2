/**
 * What the app knows about the plugins installed beside it.
 *
 * Devboule ships without Polis and has to keep working that way, so "not
 * installed" is the ordinary state and says nothing interesting. The states
 * worth telling the user apart are the other three: installed and verified,
 * installed and **refused** — the manifest and the files disagree — and a
 * plugins directory the app could not read at all.
 *
 * That third one is the reason this file exists rather than a boolean. An
 * unreadable directory reported as "nothing installed" tells someone who just
 * installed a plugin that they did not, which sends them to reinstall the one
 * thing that was never the problem.
 */

import type { PluginEntry, PluginInventory } from "../types/ipc";

/** The id Polis is installed under, and the first segment of its asset URLs. */
export const POLIS_PLUGIN_ID = "polis";

export type PluginState =
  | { kind: "ready"; entry: PluginEntry }
  | { kind: "refused"; entry: PluginEntry }
  | { kind: "absent" }
  | { kind: "unknown"; problem: string };

/**
 * Where one plugin stands.
 *
 * An entry for the id wins over a problem with the directory: the scan can
 * report both — it lists what it managed to read and says the list may be
 * short — and a plugin it did reach has a definite answer.
 */
export function pluginState(inventory: PluginInventory, id: string): PluginState {
  const entry = inventory.plugins.find((plugin) => plugin.id === id);
  if (entry) {
    return entry.ready && entry.uiEntry !== null
      ? { kind: "ready", entry }
      : { kind: "refused", entry };
  }
  if (inventory.problem) return { kind: "unknown", problem: inventory.problem };
  return { kind: "absent" };
}

/** One line a person can read, naming the plugin it is about. */
export function describePluginState(state: PluginState, id: string): string {
  switch (state.kind) {
    case "ready": {
      const { name, version } = state.entry;
      return withPayloadClamp(
        `${name ?? id} ${version ?? ""}`.trim() + " is installed and verified",
        state.entry,
      );
    }
    case "refused": {
      const reason =
        state.entry.reason ??
        (state.entry.ready && state.entry.uiEntry === null
          ? "ready but did not declare a UI entry path"
          : "no reason reported");
      return withPayloadClamp(`${id} is installed but was refused — ${reason}`, state.entry);
    }
    case "unknown":
      return `Devboule could not tell whether ${id} is installed — ${state.problem}`;
    case "absent":
      return `${id} is not installed`;
  }
}

/**
 * Host payload budgets are 1024-based (`16 * 1024 * 1024` is 16 MiB). The
 * existing size helpers (`formatMegabytes`, `humanSize`) divide by 1000 and
 * would print that as 16.8 MB.
 */
function formatPayloadBytes(bytes: number): string {
  const mib = 1024 * 1024;
  const kib = 1024;
  if (bytes >= mib && bytes % mib === 0) return `${bytes / mib} MiB`;
  if (bytes >= kib && bytes % kib === 0) return `${bytes / kib} KiB`;
  return `${bytes} bytes`;
}

/** `null` is not `false`: only an explicit clamp gets a sentence. */
function withPayloadClamp(line: string, entry: PluginEntry): string {
  if (entry.payloadBudgetClamped !== true) return line;
  if (entry.maxPayloadBytes === null) {
    return `${line}. It asked for more than the host grants.`;
  }
  return `${line}. It asked for more than the host grants, so it got ${formatPayloadBytes(
    entry.maxPayloadBytes,
  )}.`;
}

/**
 * The tone the readout should carry. Absent is deliberately neutral: shipping
 * without a plugin is not a degraded state, it is the product.
 */
export function pluginTone(state: PluginState): "ready" | "blocked" | "unknown" {
  switch (state.kind) {
    case "ready":
      return "ready";
    case "refused":
      return "blocked";
    case "unknown":
    case "absent":
      return "unknown";
  }
}
