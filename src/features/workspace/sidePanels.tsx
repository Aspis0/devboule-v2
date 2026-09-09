import { memo } from "react";
import { MOCK_DIFF_LINES, MOCK_SHIP_STEPS } from "./mockData";
import { DesignPreviewPanel } from "../design/DesignPreviewPanel";

export const ChangesSurface = memo(function ChangesSurface() {
  return (
    <div>
      <div className="workspace-changes-mockup-note" role="note">
        Mockup — these rows are hardcoded examples. Real git integration is not built yet.
      </div>
      <div className="workspace-file-changes">
        <div className="workspace-file-change workspace-file-change-selected">
          <span>index_writer.rs</span>
          <span>+92 −41</span>
        </div>
        <div className="workspace-file-change">
          <span>embedder.rs</span>
          <span>+14 −3</span>
        </div>
        <div className="workspace-file-change workspace-file-change-muted">
          <span>writer.ts</span>
          <span>deleted</span>
        </div>
      </div>

      <div className="workspace-diff-card">
        <div className="workspace-diff-header">
          <span>oracle-core/src/index_writer.rs</span>
          <span>@@ 118</span>
        </div>
        <div className="workspace-diff-lines">
          {MOCK_DIFF_LINES.map((line, index) => (
            <div
              className={`workspace-diff-line workspace-diff-${line.kind}`}
              key={`${line.line}-${index}`}
            >
              <span>{line.line}</span>
              <span>{line.text}</span>
            </div>
          ))}
        </div>
      </div>

      <div className="workspace-test-card">
        <div className="workspace-test-heading">
          <span className="workspace-status-dot workspace-dot-green" />
          <span>cargo test</span>
          <span className="workspace-test-result">142 passed</span>
        </div>
        <div className="workspace-test-meta">oracle-core 96 · devboule-mcp 46 · 8.41 s</div>
      </div>
    </div>
  );
});

export const FilesSurface = memo(function FilesSurface() {
  return (
    <div className="workspace-files-tree">
      <div className="workspace-changes-mockup-note" role="note">
        Mockup — these files are hardcoded examples. No workspace file tree is read yet.
      </div>
      <div>oracle-core/</div>
      <div className="workspace-tree-file workspace-tree-file-selected">index_writer.rs</div>
      <div className="workspace-tree-file">embedder.rs</div>
      <div className="workspace-tree-file">lance/mod.rs</div>
      <div>devboule-mcp/</div>
      <div className="workspace-tree-file">tools.rs</div>
    </div>
  );
});

interface AppSurfaceProps {
  appBuild: number;
  onReload: () => void;
}

export const AppSurface = memo(function AppSurface({ appBuild, onReload }: AppSurfaceProps) {
  return (
    <div>
      <div className="workspace-changes-mockup-note" role="note">
        Mockup — this browser page is a static example. The dev-server preview is not built yet.
      </div>
      <div className="workspace-browser-card">
        <div className="workspace-browser-toolbar">
          <span className="workspace-browser-dots">
            <span />
            <span />
          </span>
          <span className="workspace-browser-address">web.rust-core.devboule.localhost</span>
          <button
            type="button"
            className="workspace-browser-reload"
            onClick={onReload}
            title="Reload"
          >
            ↻
          </button>
        </div>
        <div className="workspace-browser-page">
          <div className="workspace-browser-title-row">
            <span className="workspace-browser-mark" />
            <span className="workspace-browser-title">Index browser</span>
            <span className="workspace-browser-build">build {appBuild}</span>
          </div>
          <div className="workspace-browser-skeleton">
            <div />
            <div className="workspace-skeleton-82" />
            <div className="workspace-skeleton-64" />
            <div className="workspace-skeleton-74" />
          </div>
        </div>
      </div>
      <div className="workspace-browser-status">
        <span className="workspace-status-dot workspace-dot-green" />
        vite dev · hot reload on agent write
      </div>
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

interface PullRequestSurfaceProps {
  prLabel: string;
  onOpen: () => void;
}

export const PullRequestSurface = memo(function PullRequestSurface({
  prLabel,
  onOpen,
}: PullRequestSurfaceProps) {
  return (
    <div>
      <div className="workspace-pr-summary">
        <div className="workspace-pr-meta-row">
          <span className="workspace-pr-status">draft</span>
          <span className="workspace-pr-number">#412</span>
        </div>
        <div className="workspace-pr-title">Move the Oracle index writer to Rust</div>
        <div className="workspace-pr-copy">
          Async flush, batched LanceDB add, TS writer deleted. Bench: 1 400 chunks/s vs 310.
        </div>
      </div>
      <div className="workspace-ship-card">
        <div className="workspace-ship-label">Ship</div>
        <div className="workspace-ship-steps">
          {MOCK_SHIP_STEPS.map((step, index) => (
            <span className="workspace-ship-step" key={step}>
              <span
                className={`workspace-ship-ring${index < 4 ? " workspace-ship-ring-active" : ""}${index < 3 ? " workspace-ship-fill-active" : index === 3 ? " workspace-ship-fill-current" : ""}`}
              />
              <span
                className={`workspace-ship-step-name${index < 4 ? " workspace-ship-step-active" : ""}`}
              >
                {step}
              </span>
            </span>
          ))}
        </div>
      </div>
      <button type="button" className="workspace-open-pr" onClick={onOpen}>
        {prLabel}
      </button>
    </div>
  );
});
