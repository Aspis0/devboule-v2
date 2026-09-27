import { memo } from "react";
import { DesignPreviewPanel } from "../design/DesignPreviewPanel";

// The Changes panel lives in `ChangesSurface.tsx` and the Files panel in
// `FilesSurface.tsx`: each carries its own data source, so neither is one of
// these placeholders.

export const AppSurface = memo(function AppSurface() {
  return (
    <div className="workspace-panel-empty">
      <h2 className="workspace-panel-empty-title">Interactive app</h2>
      <p className="workspace-panel-empty-intro">The running app preview will live here.</p>
      <p className="workspace-panel-empty-note">This panel is not available yet.</p>
    </div>
  );
});

export const DesignPanel = memo(function DesignPanel() {
  // memo is load-bearing, not a no-op: Workspace re-renders on every mousemove during a
  // panel resize drag (rightWidth feeds both style.width and aria-valuenow), and memo is
  // what keeps that drag from re-rendering this subtree. Do not remove it.
  // The body is owned by the Design feature: it mirrors the live design session
  // from the app store, not Workspace mock data.
  return <DesignPreviewPanel />;
});

export const PullRequestSurface = memo(function PullRequestSurface() {
  return (
    <div className="workspace-panel-empty">
      <h2 className="workspace-panel-empty-title">Pull request</h2>
      <p className="workspace-panel-empty-intro">The pull request summary will live here.</p>
      <p className="workspace-panel-empty-note">This panel is not available yet.</p>
    </div>
  );
});
