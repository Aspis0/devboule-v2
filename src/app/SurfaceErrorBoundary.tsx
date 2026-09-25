import { Component, Fragment, type ErrorInfo, type ReactNode } from "react";
import "./errorBoundaries.css";

interface SurfaceErrorBoundaryProps {
  children: ReactNode;
  /** Human surface name shown in the fallback, e.g. "Workspace". */
  surfaceLabel: string;
  /** A caught error clears when this changes — navigation, never a retry. */
  resetKey?: unknown;
}

interface SurfaceErrorBoundaryState {
  error: string | null;
  attempt: number;
}

// Translates Paseo's SurfaceErrorBoundary
// (packages/app/src/plugins/surface-error-boundary.tsx:23-56) to React DOM:
// one broken surface degrades alone while the shell and the other surfaces
// keep working, Retry remounts that surface only, and a resetKey change clears
// the error the way Paseo's installation/resetKey/Surface comparison does
// (:31-41). Paseo's default fallback prints the raw message as the sentence
// (:45); ours keeps the message in a details box, per this app's ErrorText
// convention that raw exception text never reads as the sentence.
export class SurfaceErrorBoundary extends Component<
  SurfaceErrorBoundaryProps,
  SurfaceErrorBoundaryState
> {
  state: SurfaceErrorBoundaryState = { error: null, attempt: 0 };

  static getDerivedStateFromError(error: unknown): Partial<SurfaceErrorBoundaryState> {
    return { error: error instanceof Error ? error.message : String(error) };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.warn(
      "[SurfaceErrorBoundary] Surface render failed",
      this.props.surfaceLabel,
      error,
      info.componentStack,
    );
  }

  componentDidUpdate(previous: SurfaceErrorBoundaryProps): void {
    // Props only — never state this boundary wrote, or the reset loops.
    if (this.state.error !== null && previous.resetKey !== this.props.resetKey) {
      this.setState({ error: null });
    }
  }

  private retry = (): void => {
    // Remount the surface children without touching the store or the rest of
    // the app. A module-level store survives the reset, so a retry can
    // re-throw at once — that shows the fallback again, never a loop, because
    // nothing retries by itself.
    this.setState((state) => ({ error: null, attempt: state.attempt + 1 }));
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
    return <Fragment key={this.state.attempt}>{this.props.children}</Fragment>;
  }
}
