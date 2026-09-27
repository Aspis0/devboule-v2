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
  | "about";

export interface SettingsMenuPage {
  id: SettingsPageId;
  label: string;
  /**
   * One line saying what the page will hold. Only pages with no function
   * carry one — a page with an intro renders the honest empty state.
   */
  intro?: string;
  /** An extra quiet line under the empty state, for the one behaviour the
      page can already name (the notification toasts). */
  note?: string;
}

export interface SettingsMenuGroup {
  label: string;
  pages: SettingsMenuPage[];
  /** The Devices group carries the live host row above its pages. */
  host?: boolean;
}

export const SETTINGS_MENU: readonly SettingsMenuGroup[] = [
  {
    label: "This machine",
    pages: [
      { id: "appearance", label: "Appearance" },
      { id: "layout", label: "Layout", intro: "How the app arranges its panes and windows." },
      { id: "editing", label: "Editing", intro: "How composing and editing messages behaves." },
      {
        id: "shortcuts",
        label: "Shortcuts",
        intro: "Keyboard shortcuts for working in the app.",
      },
      {
        id: "notifications",
        label: "Notifications",
        intro: "Sounds and toasts for things that need attention.",
        note: "Attention toasts already appear, but they have no control yet.",
      },
      { id: "diagnostics", label: "Diagnostics" },
    ],
  },
  {
    label: "Providers & agents",
    pages: [
      { id: "providers", label: "Providers" },
      { id: "profiles", label: "Agent profiles" },
      { id: "usage", label: "Usage", intro: "How much of each provider plan has been used." },
    ],
  },
  {
    label: "Workspace",
    pages: [
      { id: "projects", label: "Projects" },
      { id: "oracle", label: "Oracle" },
    ],
  },
  {
    label: "Devices",
    host: true,
    pages: [
      { id: "paired", label: "Paired devices" },
      {
        id: "permissions",
        label: "Permissions",
        intro: "What agents and paired devices may do without asking.",
      },
    ],
  },
  {
    label: "About",
    pages: [
      {
        id: "about",
        label: "About devboule",
        intro: "The app version and where to read more about it.",
      },
    ],
  },
];

/** Every page id in menu order, for the menu's arrow-key travel. */
export const SETTINGS_PAGE_ORDER: readonly SettingsPageId[] = SETTINGS_MENU.flatMap((group) =>
  group.pages.map((page) => page.id),
);

export function settingsPageById(id: SettingsPageId): SettingsMenuPage {
  const found = SETTINGS_MENU.flatMap((group) => group.pages).find((page) => page.id === id);
  if (!found) throw new Error(`Unknown settings page: ${id}`);
  return found;
}
