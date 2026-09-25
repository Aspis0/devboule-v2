import { Component, type ErrorInfo, type ReactNode } from "react";
import "./errorBoundaries.css";

interface RootErrorBoundaryProps {
  children: ReactNode;
  onReload: () => void;
}

interface RootErrorBoundaryState {
  error: string | null;
}

// Paseo formats any caught value into renderable text (root-error-details.ts).
// Ours only ever sees render-phase throws, so a small local formatter covers it.
function formatRenderError(value: unknown): string {
  if (value instanceof Error) {
    const headline = `${value.name}: ${value.message}`;
    const stack = typeof value.stack === "string" ? value.stack.trim() : "";
    if (stack === "" || stack === headline || stack.startsWith(`${headline}\n`)) {
      return stack === "" ? headline : stack;
    }
    return `${headline}\n${stack}`;
  }
  try {
    if (typeof value === "string") return value;
    return JSON.stringify(value) ?? String(value);
  } catch {
    return "[Unrenderable error value]";
  }
}

// Translates Paseo's RootErrorBoundary
// (packages/app/src/components/root-error-boundary.tsx:25-43): the whole app
// sits below it, Reload remounts through a generation key owned by the caller
// (Paseo's root-app.tsx:20-32), and the fallback carries the details because a
// packaged Tauri window has no visible console to read them from.
export class RootErrorBoundary extends Component<RootErrorBoundaryProps, RootErrorBoundaryState> {
  state: RootErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): Partial<RootErrorBoundaryState> {
    return { error: formatRenderError(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    // Console only, as Paseo does — it has no remote reporting either. The log
    // carries the error and its component stack and nothing else.
    console.error("[RootErrorBoundary] Unhandled render error", {
      error: formatRenderError(error),
      componentStack: info.componentStack,
    });
  }

  render(): ReactNode {
    if (this.state.error !== null) {
      return <RootErrorFallback error={this.state.error} onReload={this.props.onReload} />;
    }
    return this.props.children;
  }
}

interface RootErrorFallbackProps {
  error: string;
  onReload: () => void;
}

// Paseo's copy (packages/app/src/i18n/resources/en.ts:1478-1481) with our
// name; the Reload button and the capped details box keep their meaning.
export function RootErrorFallback({ error, onReload }: RootErrorFallbackProps): ReactNode {
  return (
    <div className="root-fallback" role="alert">
      <div className="root-fallback-card">
        <h1 className="root-fallback-title">Devboule ran into a problem.</h1>
        <p className="root-fallback-body">
          Reload the app to try again. If this keeps happening, include the details below when you
          report it.
        </p>
        <h2 className="root-fallback-details-label">Details</h2>
        <pre className="root-fallback-details">{error}</pre>
        <button type="button" className="boundary-reload" onClick={onReload}>
          Reload
        </button>
      </div>
    </div>
  );
}
