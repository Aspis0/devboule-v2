/**
 * Settings navigation (real) + device placeholders (mock).
 *
 * MOCK_SETTINGS_TABS is not a mock: it is the real navigation for the
 * Settings surface. Add or remove an entry here when a tab comes or goes.
 *
 * MOCK_DEVICES is still a placeholder: no device or pairing IPC exists
 * behind it yet (no device/pairing command in src-tauri, no device wrapper
 * in src/lib/tauri.ts), so these two rows stay hardcoded until a typed
 * devices IPC response replaces them.
 */

export type SettingsTab =
  | "general"
  | "projects"
  | "oracle"
  | "providers"
  | "devices"
  | "diagnostics";

export const MOCK_SETTINGS_TABS: readonly { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "projects", label: "Projects" },
  { id: "oracle", label: "Oracle" },
  { id: "providers", label: "Providers & models" },
  { id: "devices", label: "Devices" },
  { id: "diagnostics", label: "Diagnostics" },
];

export const MOCK_DEVICES = [
  { name: "this mac · admin", state: "trust anchor", tone: "ready" },
  { name: "iphone · read + steer", state: "last seen 2 h", tone: "idle" },
] as const;
