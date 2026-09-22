import type { ReactNode } from "react";
import { AppSurface, DesignPanel, PullRequestSurface } from "./sidePanels";
import { ChangesSurface } from "./ChangesSurface";
import { FilesSurface } from "./FilesSurface";
import { CHANGES_BADGE_UNREAD, changesBadge } from "./changesBadge";

export type DotTone = "terracotta" | "silence" | "green" | "purple" | "ochre";

export interface SidePanelContext {
  appBuild: number;
  onReload: () => void;
  prLabel: string;
  onOpenPullRequest: () => void;
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
 * read — the registry then falls back to the entry's static `meta`.
 */
export interface SidePanelLiveMeta {
  subscribe: (listener: () => void) => () => void;
  snapshot: (workspaceId: string | null) => string | null;
}

export interface SidePanelEntry {
  id: string;
  name: string;
  /** The badge beside the panel's name; a panel with live data overrides it. */
  meta: string;
  liveMeta?: SidePanelLiveMeta;
  dotTone: DotTone;
  render: (context: SidePanelContext) => ReactNode;
}

// Keep panel composition here so future plugin panels can contribute an entry without reopening
// Workspace; this is intentionally not a plugin registration API.
export const SIDE_PANEL_REGISTRY: readonly SidePanelEntry[] = [
  {
    id: "changes",
    name: "Changes",
    // Before the first read, and what a workspace never read shows (DECISIONS §9:
    // the open panel's poll supplies the real label, a closed panel keeps it).
    meta: CHANGES_BADGE_UNREAD,
    liveMeta: changesBadge,
    dotTone: "terracotta",
    render: ({ workspaceId }) => <ChangesSurface workspaceId={workspaceId} />,
  },
  // The Files panel reads a real tree, but has no live badge: its meta is a
  // property of the panel, never an invented count (an example of one, the
  // old `2 140`, was exactly the defect this comment used to carry).
  {
    id: "files",
    name: "Files",
    meta: "read-only",
    dotTone: "silence",
    render: ({ workspaceId }) => <FilesSurface workspaceId={workspaceId} />,
  },
  // This metadata is a mockup: it is an invented value for the mock Interactive app panel.
  {
    id: "app",
    name: "Interactive app",
    meta: "localhost",
    dotTone: "green",
    render: ({ appBuild, onReload }) => <AppSurface appBuild={appBuild} onReload={onReload} />,
  },
  {
    id: "design",
    name: "Design",
    meta: "session mirror",
    dotTone: "purple",
    render: () => <DesignPanel />,
  },
  // This metadata is a mockup: it is an invented value for the mock Pull request panel.
  {
    id: "pr",
    name: "Pull request",
    meta: "#412",
    dotTone: "ochre",
    render: ({ prLabel, onOpenPullRequest }) => (
      <PullRequestSurface prLabel={prLabel} onOpen={onOpenPullRequest} />
    ),
  },
];
