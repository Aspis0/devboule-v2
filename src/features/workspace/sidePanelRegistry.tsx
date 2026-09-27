import type { ReactNode } from "react";
import { AppSurface, DesignPanel, PullRequestSurface } from "./sidePanels";
import { ChangesSurface } from "./ChangesSurface";
import { FilesSurface } from "./FilesSurface";
import { changesBadge } from "./changesBadge";
import type { PanelIconName } from "./panel/PanelIcon";

/** Where the tab row offers a panel: a visible tab, or the kebab menu. The
 * three spec tabs stay tabs; mock and future panels must not crowd them. */
export type PanelPlacement = "tab" | "menu";

export interface SidePanelContext {
  /**
   * The selected workspace's id — the only form of it that may leave this
   * process (`src/types/ipc.ts` declares `Workspace.path` display-only).
   */
  workspaceId: string | null;
}

/**
 * A panel whose own reads produce the badge, instead of a value written into
 * the registry. The snapshot is keyed by workspace so one checkout's numbers
 * can never appear under another's name, and `null` means this panel has never
 * read — the tab then shows the unread mark.
 */
export interface SidePanelLiveMeta {
  subscribe: (listener: () => void) => () => void;
  snapshot: (workspaceId: string | null) => string | null;
}

export interface SidePanelEntry {
  id: string;
  name: string;
  placement: PanelPlacement;
  icon: PanelIconName;
  liveMeta?: SidePanelLiveMeta;
  render: (context: SidePanelContext) => ReactNode;
}

// Keep panel composition here so future plugin panels can contribute an entry without reopening
// Workspace; this is intentionally not a plugin registration API.
export const SIDE_PANEL_REGISTRY: readonly SidePanelEntry[] = [
  {
    id: "files",
    name: "Files",
    placement: "tab",
    icon: "files",
    render: ({ workspaceId }) => <FilesSurface workspaceId={workspaceId} />,
  },
  {
    id: "changes",
    name: "Changes",
    placement: "tab",
    icon: "changes",
    // The open panel's poll supplies the label R7b's branch row reads; the
    // tab row itself carries no badge (no room at 300 px).
    liveMeta: changesBadge,
    render: ({ workspaceId }) => <ChangesSurface workspaceId={workspaceId} />,
  },
  {
    id: "design",
    name: "Design",
    placement: "tab",
    icon: "design",
    render: () => <DesignPanel />,
  },
  {
    id: "app",
    name: "Interactive app",
    placement: "menu",
    icon: "app",
    render: () => <AppSurface />,
  },
  {
    id: "pr",
    name: "Pull request",
    placement: "menu",
    icon: "pr",
    render: () => <PullRequestSurface />,
  },
];
