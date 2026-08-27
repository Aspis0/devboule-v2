/** M1b mock boundary. Replace these values with typed settings IPC responses. */

export type SettingsTab = "general" | "projects" | "oracle" | "providers" | "devices" | "labs";
export interface MockProvider {
  id: "claude" | "codex" | "opencode" | "local";
  name: string;
  initial: string;
  tone: "purple" | "terracotta" | "green" | "ochre";
  detail: string;
  installed: boolean;
  authenticated: boolean;
  enabled: boolean;
}

export type ProviderId = MockProvider["id"];

export const MOCK_PROVIDERS: readonly MockProvider[] = [
  {
    id: "claude",
    name: "Claude Code",
    initial: "C",
    tone: "purple",
    detail: "claude · 2.1.4 · subscription",
    installed: true,
    authenticated: true,
    enabled: true,
  },
  {
    id: "codex",
    name: "Codex CLI",
    initial: "X",
    tone: "terracotta",
    detail: "codex · 0.9.2 · api key",
    installed: true,
    authenticated: true,
    enabled: true,
  },
  {
    id: "opencode",
    name: "OpenCode",
    initial: "O",
    tone: "green",
    detail: "opencode · not on PATH",
    installed: false,
    authenticated: false,
    enabled: false,
  },
  {
    id: "local",
    name: "Local · Ollama",
    initial: "L",
    tone: "ochre",
    detail: "qwen3-8b, bge-small · loopback only",
    installed: true,
    authenticated: true,
    enabled: true,
  },
];

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

export const MOCK_DEFAULT_MODELS = [
  { title: "Workspace composer", value: "claude / sonnet-4.6" },
  { title: "Oracle answers", value: "local / qwen3-8b" },
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

export const MOCK_PROVIDER_ENABLED: Record<ProviderId, boolean> = Object.fromEntries(
  MOCK_PROVIDERS.map((provider) => [provider.id, provider.enabled]),
) as Record<ProviderId, boolean>;
