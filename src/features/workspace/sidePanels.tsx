import { memo } from 'react';
import { MOCK_DIFF_LINES, MOCK_SHIP_STEPS } from './mockData';

export type DiffState = 'unstaged' | 'staged' | 'discarded';

const DIFF_LABELS: Record<DiffState, string> = {
  unstaged: 'Unstaged · 3 hunks',
  staged: 'Staged',
  discarded: 'Discarded',
};

interface ChangesSurfaceProps {
  diffState: DiffState;
  onDiffStateChange: (state: DiffState) => void;
}

export const ChangesSurface = memo(function ChangesSurface({ diffState, onDiffStateChange }: ChangesSurfaceProps) {
  return (
    <div>
      <div className="workspace-file-changes">
        <button type="button" className="workspace-file-change workspace-file-change-selected">
          <span>index_writer.rs</span><span>+92 −41</span>
        </button>
        <button type="button" className="workspace-file-change">
          <span>embedder.rs</span><span>+14 −3</span>
        </button>
        <button type="button" className="workspace-file-change workspace-file-change-muted">
          <span>writer.ts</span><span>deleted</span>
        </button>
      </div>

      <div className="workspace-diff-card">
        <div className="workspace-diff-header">
          <span>oracle-core/src/index_writer.rs</span>
          <span>@@ 118</span>
        </div>
        <div className="workspace-diff-lines">
          {MOCK_DIFF_LINES.map((line, index) => (
            <div className={`workspace-diff-line workspace-diff-${line.kind}`} key={`${line.line}-${index}`}>
              <span>{line.line}</span><span>{line.text}</span>
            </div>
          ))}
        </div>
        <div className="workspace-diff-actions">
          <span className="workspace-diff-status">{DIFF_LABELS[diffState]}</span>
          <button
            type="button"
            className="workspace-secondary-action workspace-discard-action"
            onClick={() => onDiffStateChange('discarded')}
          >
            Discard
          </button>
          <button
            type="button"
            className="workspace-primary-action"
            onClick={() => onDiffStateChange('staged')}
          >
            Stage
          </button>
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
      <div>oracle-core/</div>
      <button type="button" className="workspace-tree-file workspace-tree-file-selected">index_writer.rs</button>
      <button type="button" className="workspace-tree-file">embedder.rs</button>
      <button type="button" className="workspace-tree-file">lance/mod.rs</button>
      <div>devboule-mcp/</div>
      <button type="button" className="workspace-tree-file">tools.rs</button>
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
      <div className="workspace-browser-card">
        <div className="workspace-browser-toolbar">
          <span className="workspace-browser-dots"><span /><span /></span>
          <span className="workspace-browser-address">web.rust-core.devboule.localhost</span>
          <button type="button" className="workspace-browser-reload" onClick={onReload} title="Reload">↻</button>
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
          <div className="workspace-browser-actions">
            <button type="button" className="workspace-browser-primary">Reindex</button>
            <button type="button" className="workspace-browser-secondary">Export</button>
          </div>
        </div>
      </div>
      <div className="workspace-browser-status"><span className="workspace-status-dot workspace-dot-green" />vite dev · hot reload on agent write</div>
    </div>
  );
});

export const DesignPanel = memo(function DesignPanel() {
  return (
    <div>
      <div className="workspace-grounding-row">
        <span className="workspace-status-dot workspace-dot-green" />
        <span>Grounded · devboule</span>
        <button type="button" className="workspace-open-design">Open Design</button>
      </div>
      <div className="workspace-generation-label">1 generation</div>
      <div className="workspace-generation-cards">
        <div className="workspace-generation-card">
          <div className="workspace-generation-heading">
            <span className="workspace-generation-icon">✓</span>
            <span>Edited Index header</span>
          </div>
          <div className="workspace-generation-copy">Pulled the count from the real hygiene snapshot and removed the duplicate action. Radius and shadow snapped to radius.md / shadow.soft.</div>
          <div className="workspace-generation-sources">
            <span>WorkspaceView.tsx</span>
            <span>oracle-core/src/classify.rs</span>
            <span>tokens.json</span>
          </div>
        </div>
      </div>
      <div className="workspace-design-composer">
        <textarea placeholder="Describe what to generate…" rows={2} aria-label="Describe what to generate" />
        <div className="workspace-design-composer-footer">
          <span>Claude Code · High</span>
          <button type="button" className="workspace-primary-action">Generate</button>
        </div>
      </div>
      <div className="workspace-design-note">Generations land on the Design canvas in this worktree; Save to repo writes them back as components.</div>
    </div>
  );
});

interface PullRequestSurfaceProps {
  prLabel: string;
  onOpen: () => void;
}

export const PullRequestSurface = memo(function PullRequestSurface({ prLabel, onOpen }: PullRequestSurfaceProps) {
  return (
    <div>
      <div className="workspace-pr-summary">
        <div className="workspace-pr-meta-row">
          <span className="workspace-pr-status">draft</span>
          <span className="workspace-pr-number">#412</span>
        </div>
        <div className="workspace-pr-title">Move the Oracle index writer to Rust</div>
        <div className="workspace-pr-copy">Async flush, batched LanceDB add, TS writer deleted. Bench: 1 400 chunks/s vs 310.</div>
      </div>
      <div className="workspace-ship-card">
        <div className="workspace-ship-label">Ship</div>
        <div className="workspace-ship-steps">
          {MOCK_SHIP_STEPS.map((step, index) => (
            <span className="workspace-ship-step" key={step}>
              <span className={`workspace-ship-ring${index < 4 ? ' workspace-ship-ring-active' : ''}${index < 3 ? ' workspace-ship-fill-active' : index === 3 ? ' workspace-ship-fill-current' : ''}`} />
              <span className={`workspace-ship-step-name${index < 4 ? ' workspace-ship-step-active' : ''}`}>{step}</span>
            </span>
          ))}
        </div>
      </div>
      <button type="button" className="workspace-open-pr" onClick={onOpen}>{prLabel}</button>
    </div>
  );
});
