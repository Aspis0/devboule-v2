import { Component, type ErrorInfo, type ReactNode } from "react";
import "./errorBoundaries.css";

interface RootErrorBoundaryProps {
  children: ReactNode;
}

interface RootErrorBoundaryState {
  error: string | null;
}

// Paseo formats any caught value into renderable text
// (packages/app/src/components/root-error-details.ts); ours only ever sees
// render-phase throws, so a small local formatter covers it.
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

// Translated from Paseo's RootErrorBoundary
// (packages/app/src/components/root-error-boundary.tsx): the whole app sits
// below it, and the fallback carries the details because a packaged Tauri
// window has no visible console to read them from. Reload is a real document
// reload: a key-bump remount cannot recover a failed lazy() import — React
// caches the rejection forever — so there is exactly one recovery mechanism.
export class RootErrorBoundary extends Component<RootErrorBoundaryProps, RootErrorBoundaryState> {
  state: RootErrorBoundaryState = { error: null };

  static getDerivedStateFromError(error: unknown): Partial<RootErrorBoundaryState> {
    return { error: formatRenderError(error) };
  }

  componentDidCatch(error: unknown, info: ErrorInfo): void {
    // Console only — no remote reporting. The log carries the error and its
    // component stack and nothing else.
    console.error("[RootErrorBoundary] Unhandled render error", {
      error: formatRenderError(error),
      componentStack: info.componentStack,
    });
  }

  render(): ReactNode {
    if (this.state.error !== null) {
      return <RootErrorFallback error={this.state.error} />;
    }
    return this.props.children;
  }
}

// Paseo's copy (packages/app/src/i18n/resources/en.ts) with our name; the
// last body sentence is ours: a document reload always boots the default
// surface, and the copy must say where the user lands.
function RootErrorFallback({ error }: { error: string }): ReactNode {
  return (
    <div className="root-fallback" role="alert">
      <div className="root-fallback-card">
        <h1 className="root-fallback-title">Devboule ran into a problem.</h1>
        <p className="root-fallback-body">
          Reload restarts the app on the Workspace surface — unsent drafts are lost. If this keeps
          happening, include the details below when you report it.
        </p>
        <h2 className="root-fallback-details-label">Details</h2>
        <pre className="root-fallback-details">{error}</pre>
        {/* Paseo's compact-footer idiom
            (packages/app/src/components/root-error-boundary.tsx): Reload is
            pinned so it never scrolls away at high zoom. */}
        <div className="root-fallback-footer">
          <button
            type="button"
            className="boundary-reload"
            onClick={() => window.location.reload()}
          >
            Reload
          </button>
        </div>
      </div>
    </div>
  );
}
