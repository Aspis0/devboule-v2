/** M1b mock boundary. Replace these values with typed settings IPC responses. */

export type SettingsTab = "general" | "projects" | "oracle" | "providers" | "devices" | "labs";

export const MOCK_SETTINGS_TABS: readonly { id: SettingsTab; label: string }[] = [
  { id: "general", label: "General" },
  { id: "projects", label: "Projects" },
  { id: "oracle", label: "Oracle" },
  { id: "providers", label: "Providers & models" },
  { id: "devices", label: "Devices" },
  { id: "labs", label: "Labs" },
];

export const MOCK_PROJECTS = [
  {
    name: "devboule",
    path: "~/dev/devboule · github.com/Aspis0/devboule",
    workspaces: "3 workspaces",
  },
  {
    name: "pubvia",
    path: "~/papers/somatic-clones · local only",
    workspaces: "1 workspace",
  },
] as const;

export const MOCK_WORKTREE_DEFAULTS = [
  { label: "Base branch", value: "origin/main", tone: "default" },
  { label: "Setup script", value: "cargo fetch && npm ci", tone: "default" },
  { label: "Remove worktree when archived", value: "on", tone: "green" },
] as const;

export const MOCK_DEVICES = [
  { name: "this mac · admin", state: "trust anchor", tone: "ready" },
  { name: "iphone · read + steer", state: "last seen 2 h", tone: "idle" },
] as const;

export const MOCK_GENERAL_SETTINGS = [
  { label: "Crescent reveal zone", value: "top centre · 13 px sliver", tone: "default" },
  { label: "Default send", value: "enter sends ▾", tone: "default" },
  { label: "Daemon shuts down with the app", value: "off", tone: "danger" },
  { label: "Telemetry", value: "none, ever", tone: "muted" },
] as const;

// Pigeon dispatcher and Censor were v1 subsystems. v2 dropped both and does not
// plan to bring them back, so Labs must not advertise them.
export const MOCK_LABS = [
  { title: "Generative design", description: "Draw UI from a prompt inside a workspace." },
] as const;
