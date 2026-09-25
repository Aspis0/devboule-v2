import type { ReactNode } from "react";
import { App } from "./App";
import { RootErrorBoundary } from "./RootErrorBoundary";

// The whole app under its root error boundary. The boundary lives here —
// above App, not inside it — so a throw in App's own render or effects still
// reaches it; main.tsx renders this, never App directly.
export function AppRoot(): ReactNode {
  return (
    <RootErrorBoundary>
      <App />
    </RootErrorBoundary>
  );
}
