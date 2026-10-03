import type { ReactNode } from "react";
import { DesignPanel, PullRequestSurface } from "./sidePanels";
import { ChangesSurface } from "./ChangesSurface";
import { FilesSurface } from "./FilesSurface";
import type { PanelIconName } from "./panel/PanelIcon";
import type { WorkspaceKey } from "./hosts/hostIdentity";
import type { FileToolTabKind } from "./strip/toolTabs";

/** Where the tab row offers a panel: a visible tab, or the kebab menu. The
 * three spec tabs stay tabs; mock and future panels must not crowd them. */
export type PanelPlacement = "tab" | "menu";

export interface SidePanelContext {
  /**
   * The selected workspace as the UI names it: which panel this is, what
   * every cache it fills is keyed by, and what it resolves to a daemon id
   * where it calls one.
   */
  workspaceKey: WorkspaceKey | null;
  /**
   * Whether the running daemon can list a workspace's history — the one
   * fact the Changes panel gates on, as a primitive so the surface's
   * memo holds. Workspace computes it from the status it already holds.
   */
  canListCommits: boolean;
  /**
   * Open a path as a main tab, under the panel's workspace. Each entry binds
   * its own kind, so the kind travels only this far.
   */
  onOpenFile?: (workspaceKey: WorkspaceKey, path: string, kind: FileToolTabKind) => void;
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
    render: ({ workspaceKey, onOpenFile }) => (
      <FilesSurface
        workspaceKey={workspaceKey}
        onOpenFile={
          onOpenFile === undefined
            ? undefined
            : (childWorkspaceKey, path) => onOpenFile(childWorkspaceKey, path, "file")
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
    render: ({ workspaceKey, canListCommits, onOpenFile }) => (
      <ChangesSurface
        workspaceKey={workspaceKey}
        canListCommits={canListCommits}
        onOpenFile={
          onOpenFile === undefined
            ? undefined
            : (childWorkspaceKey, path) => onOpenFile(childWorkspaceKey, path, "diff")
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
    id: "pr",
    name: "Pull request",
    placement: "menu",
    icon: "pr",
    render: () => <PullRequestSurface />,
  },
];
