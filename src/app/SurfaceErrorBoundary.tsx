import { Component, type ErrorInfo, type ReactNode } from "react";
import "./errorBoundaries.css";

interface SurfaceErrorBoundaryProps {
  children: ReactNode;
  /** Human surface name shown in the fallback, e.g. "Workspace". */
  surfaceLabel: string;
}

interface SurfaceErrorBoundaryState {
  error: string | null;
}

// Translates Paseo's SurfaceErrorBoundary
// (packages/app/src/plugins/surface-error-boundary.tsx:23-56) to React DOM:
// one broken surface degrades alone while the shell and the other surfaces
// keep working. The only reset is the call-site key, which remounts the
// boundary on navigation; Retry only clears the error, and the children mount
// fresh anyway because React unmounts the failed subtree when it catches.
// Paseo's default fallback prints the raw message as the sentence (:45); ours
// keeps the message in a details box, per this app's ErrorText convention
// that raw exception text never reads as the sentence.
export class SurfaceErrorBoundary extends Component<
  SurfaceErrorBoundaryProps,
  SurfaceErrorBoundaryState
> {
  state: SurfaceErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): Partial<SurfaceErrorBoundaryState> {
    return { error: error instanceof Error ? error.message : String(error) };
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
    // A module-level store survives the reset, so a retry can re-throw at
    // once — that shows the fallback again, never a loop, because nothing
    // retries by itself.
    this.setState({ error: null });
  };

  render(): ReactNode {
    if (this.state.error !== null) {
      return (
        <section
          className="surface-fallback"
          role="alert"
          aria-label={`${this.props.surfaceLabel} failed to render`}
        >
          <h2 className="surface-fallback-title">{this.props.surfaceLabel} could not be shown.</h2>
          <pre className="surface-fallback-details">{this.state.error}</pre>
          <button type="button" className="boundary-retry" onClick={this.retry}>
            Retry
          </button>
        </section>
      );
    }
    return this.props.children;
  }
}
