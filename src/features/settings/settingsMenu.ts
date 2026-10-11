/** The pages of the Settings left menu, in menu order. */
export type SettingsPageId =
  | "appearance"
  | "layout"
  | "editing"
  | "shortcuts"
  | "notifications"
  | "diagnostics"
  | "providers"
  | "profiles"
  | "usage"
  | "projects"
  | "oracle"
  | "paired"
  | "permissions"
  | "saved-logins"
  | "about";

export interface SettingsMenuPage {
  id: SettingsPageId;
  label: string;
  /** Empty when the page needs no paragraph under its title; the rows say the rest. */
  intro: string;
  /**
   * Set for pages with no function yet: the page renders the quiet
   * "not available" line and no controls.
   */
  unavailable?: boolean;
  /** An extra quiet line under the empty state, for the one behaviour the
      page can already name (the notification toasts). */
  note?: string;
}

export interface SettingsMenuGroup {
  label: string;
  pages: SettingsMenuPage[];
  /** The Devices group carries the host row above its pages. */
  host?: boolean;
}

export const SETTINGS_MENU: readonly SettingsMenuGroup[] = [
  {
    label: "This machine",
    pages: [
      { id: "appearance", label: "Appearance", intro: "" },
      {
        id: "layout",
        label: "Window",
        intro: "",
      },
      {
        id: "editing",
        label: "Editing",
        intro: "",
      },
      {
        id: "shortcuts",
        label: "Shortcuts",
        intro: "",
      },
      {
        id: "notifications",
        label: "Notifications",
        intro: "",
      },
      {
        id: "diagnostics",
        label: "Diagnostics",
        intro:
          "Numbers and versions about the app. Redacted by the daemon: no secrets, no conversation text. Safe to paste into an issue.",
      },
      {
        id: "saved-logins",
        label: "Saved logins",
        intro:
          "Logins this PC may fill in for an agent. The password stays in the credential store; an agent never reads it.",
      },
    ],
  },
  {
    label: "Providers & agents",
    pages: [
      {
        id: "providers",
        label: "Providers",
        intro: "The agent CLIs this daemon can start and the tools each offers.",
      },
      {
        id: "profiles",
        label: "Agent profiles",
        intro: "",
      },
      {
        id: "usage",
        label: "Usage",
        intro: "Shown when a provider reports it; nothing is fetched.",
      },
    ],
  },
  {
    label: "Workspace",
    pages: [
      {
        id: "projects",
        label: "Projects",
        intro:
          "A project is a git repository or any folder this daemon can reach. Workspaces live inside it.",
      },
      {
        id: "oracle",
        label: "Oracle",
        intro:
          "Local code search. Ask where code lives and get the smallest useful source spans to open.",
      },
    ],
  },
  {
    label: "Devices",
    host: true,
    pages: [
      {
        id: "paired",
        label: "Paired devices",
        intro: "Paired clients that may drive this daemon. Pairing is per-device and revocable.",
      },
      {
        id: "permissions",
        label: "Permissions",
        intro: "What agents and paired devices may do without asking.",
        unavailable: true,
      },
    ],
  },
  {
    label: "About",
    pages: [
      {
        id: "about",
        label: "About devboule",
        intro: "",
      },
    ],
  },
];

/** Every page id in menu order, for the menu's arrow-key travel. */
export const SETTINGS_PAGE_ORDER: readonly SettingsPageId[] = SETTINGS_MENU.flatMap((group) =>
  group.pages.map((page) => page.id),
);

const PAGE_BY_ID: ReadonlyMap<SettingsPageId, SettingsMenuPage> = new Map(
  SETTINGS_MENU.flatMap((group) => group.pages.map((page) => [page.id, page] as const)),
);

export function settingsPageById(id: SettingsPageId): SettingsMenuPage {
  const found = PAGE_BY_ID.get(id);
  if (!found) throw new Error(`Unknown settings page: ${id}`);
  return found;
}
