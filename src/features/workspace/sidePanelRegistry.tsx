import type { ReactNode } from "react";
import {
  AppSurface,
  ChangesSurface,
  DesignPanel,
  FilesSurface,
  PullRequestSurface,
} from "./sidePanels";

export type DotTone = "terracotta" | "silence" | "green" | "purple" | "ochre";

export interface SidePanelContext {
  appBuild: number;
  onReload: () => void;
  prLabel: string;
  onOpenPullRequest: () => void;
}

export interface SidePanelEntry {
  id: string;
  name: string;
  meta: string;
  dotTone: DotTone;
  render: (context: SidePanelContext) => ReactNode;
}

// Keep panel composition here so future plugin panels can contribute an entry without reopening
// Workspace; this is intentionally not a plugin registration API.
export const SIDE_PANEL_REGISTRY: readonly SidePanelEntry[] = [
  // This metadata is a mockup: it is an invented value for the mock Changes panel.
  {
    id: "changes",
    name: "Changes",
    meta: "+118 −64",
    dotTone: "terracotta",
    render: () => <ChangesSurface />,
  },
  // This metadata is a mockup: it is an invented value for the mock Files panel.
  { id: "files", name: "Files", meta: "2 140", dotTone: "silence", render: () => <FilesSurface /> },
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
