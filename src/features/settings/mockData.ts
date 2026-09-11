/**
 * Device placeholders (mock).
 *
 * MOCK_DEVICES is still a placeholder: no device or pairing IPC exists
 * behind it yet (no device/pairing command in src-tauri, no device wrapper
 * in src/lib/tauri.ts), so these two rows stay hardcoded until a typed
 * devices IPC response replaces them.
 */

export const MOCK_DEVICES = [
  { name: "this mac · admin", state: "trust anchor", tone: "ready" },
  { name: "iphone · read + steer", state: "last seen 2 h", tone: "idle" },
] as const;
