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
        label: "Layout",
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
        intro: "Numbers and versions about the app itself, plus the transcript history it keeps.",
      },
      {
        id: "saved-logins",
        label: "Saved logins",
        intro:
          "Logins this machine may fill in for an agent. The password stays in this machine's credential store; an agent never reads it.",
      },
    ],
  },
  {
    label: "Providers & agents",
    pages: [
      {
        id: "providers",
        label: "Providers",
        intro:
          "The agent CLIs this daemon can start, and the tools each one offers. An executable is not a login: the status shows the login check when there is one, else the last start, or that it has not measured one.",
      },
      {
        id: "profiles",
        label: "Agent profiles",
        intro: "",
      },
      {
        id: "usage",
        label: "Usage",
        intro: "",
      },
    ],
  },
  {
    label: "Workspace",
    pages: [
      {
        id: "projects",
        label: "Projects",
        intro: "",
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
