import type { ReactNode } from "react";
import { AppSurface, DesignPanel, PullRequestSurface } from "./sidePanels";
import { ChangesSurface } from "./ChangesSurface";
import { FilesSurface } from "./FilesSurface";
import type { PanelIconName } from "./panel/PanelIcon";
import type { ToolTabKind } from "./strip/toolTabs";

/** Where the tab row offers a panel: a visible tab, or the kebab menu. The
 * three spec tabs stay tabs; mock and future panels must not crowd them. */
export type PanelPlacement = "tab" | "menu";

export interface SidePanelContext {
  /**
   * The selected workspace's id — the only form of it that may leave this
   * process (`src/types/ipc.ts` declares `Workspace.path` display-only).
   */
  workspaceId: string | null;
  /**
   * Whether the running daemon can list a workspace's history — the one
   * fact the Changes panel gates on, as a primitive so the surface's
   * memo holds. Workspace computes it from the status it already holds.
   */
  canListCommits: boolean;
  /**
   * Open a path as a main tab. Each entry binds its own kind — the Changes
   * entry a diff tab, the Files entry a file tab — so the trees keep their
   * (workspaceId, path) call shape and the kind travels only this far.
   */
  onOpenFile?: (workspaceId: string, path: string, kind: ToolTabKind) => void;
}

export interface SidePanelEntry {
  id: string;
  name: string;
  placement: PanelPlacement;
  icon: PanelIconName;
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
    render: ({ workspaceId, onOpenFile }) => (
      <FilesSurface
        workspaceId={workspaceId}
        onOpenFile={
          onOpenFile === undefined
            ? undefined
            : (childWorkspaceId, path) => onOpenFile(childWorkspaceId, path, "file")
        }
      />
    ),
  },
  {
    id: "changes",
    name: "Changes",
    placement: "tab",
    icon: "changes",
    // The counts live in R7b's branch row, read off the panel's own poll via
    // changesBadgeLabel — never on the tab (no room at 300 px).
    render: ({ workspaceId, canListCommits, onOpenFile }) => (
      <ChangesSurface
        workspaceId={workspaceId}
        canListCommits={canListCommits}
        onOpenFile={
          onOpenFile === undefined
            ? undefined
            : (childWorkspaceId, path) => onOpenFile(childWorkspaceId, path, "diff")
        }
      />
    ),
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
