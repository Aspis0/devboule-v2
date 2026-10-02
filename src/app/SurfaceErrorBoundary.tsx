import { Component, useEffect, useRef, type ErrorInfo, type ReactNode } from "react";
import { useCopyFeedback } from "../lib/useCopyFeedback";
import "./errorBoundaries.css";

interface SurfaceErrorBoundaryProps {
  children: ReactNode;
  /** Human surface name shown in the fallback, e.g. "Workspace". */
  surfaceLabel: string;
}

interface SurfaceErrorBoundaryState {
  detail: string | null;
}

// Translated from Paseo's SurfaceErrorBoundary
// (packages/app/src/plugins/surface-error-boundary.tsx).
export class SurfaceErrorBoundary extends Component<
  SurfaceErrorBoundaryProps,
  SurfaceErrorBoundaryState
> {
  state: SurfaceErrorBoundaryState = { detail: null };

  static getDerivedStateFromError(error: unknown): Partial<SurfaceErrorBoundaryState> {
    return { detail: formatSurfaceError(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.warn(
      "[SurfaceErrorBoundary] Surface render failed",
      this.props.surfaceLabel,
      error,
      info.componentStack,
    );
  }

  private retry = (): void => {
    // The failed subtree was unmounted when the throw was caught, so
    // clearing the error mounts a fresh child instead of re-rendering it.
    this.setState({ detail: null });
  };

  render(): ReactNode {
    if (this.state.detail !== null) {
      return (
        <SurfaceFallback
          surfaceLabel={this.props.surfaceLabel}
          detail={this.state.detail}
          onRetry={this.retry}
        />
      );
    }
    return this.props.children;
  }
}

// React lets a render throw anything, and non-Error values still need
// readable details for a bug report. The whole body is guarded because an
// Error's stack, name and message can be throwing getters.
function formatSurfaceError(value: unknown): string {
  try {
    if (value instanceof Error) {
      return typeof value.stack === "string" && value.stack.trim() !== ""
        ? value.stack
        : `${value.name}: ${value.message}`;
    }
    if (typeof value === "string") return value;
    return JSON.stringify(value) ?? String(value);
  } catch {
    return "Unrenderable error value";
  }
}

interface SurfaceFallbackProps {
  surfaceLabel: string;
  detail: string;
  onRetry: () => void;
}

function SurfaceFallback({ surfaceLabel, detail, onRetry }: SurfaceFallbackProps): ReactNode {
  const feedback = useCopyFeedback({ resetAfterMs: 1500 });
  const retryRef = useRef<HTMLButtonElement>(null);

  // When the failed subtree unmounts, focus falls to the body; focus that is
  // anywhere else in the app is not this fallback's to take.
  useEffect(() => {
    const active = document.activeElement;
    if (active === null || active === document.body) retryRef.current?.focus();
  }, []);

  return (
    <section className="surface-fallback">
      <h2 className="surface-fallback-title">
        <span role="alert">{surfaceLabel} stopped working.</span>
      </h2>
      <p className="surface-fallback-body">
        Retry starts it again. If it still fails, reload Devboule — the app restarts on the
        Workspace surface and unsent drafts are lost.
      </p>
      <details className="surface-fallback-disclosure">
        <summary className="surface-fallback-summary">Technical details</summary>
        <div className="surface-fallback-details-body">
          <pre className="surface-fallback-details">{detail}</pre>
          <button
            type="button"
            className="surface-fallback-copy"
            onClick={() => void feedback.copy("details", detail)}
          >
            {feedback.labelFor("details", "Copy details")}
          </button>
          <p className="surface-fallback-body">May include file paths from this computer.</p>
        </div>
      </details>
      <div className="surface-fallback-actions">
        <button type="button" className="boundary-retry" ref={retryRef} onClick={onRetry}>
          Retry
        </button>
        <button
          type="button"
          className="boundary-reload surface-fallback-reload"
          onClick={() => window.location.reload()}
        >
          Reload Devboule
        </button>
      </div>
    </section>
  );
}
