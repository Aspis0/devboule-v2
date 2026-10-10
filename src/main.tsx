import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { AppRoot } from "./app/AppRoot";
import { installEditorFlush } from "./features/workspace/fileEditor/editorRegistry";
import { startThemeSync } from "./lib/theme";
import "./styles/tokens.css";
import "./styles/global.css";

// Resolves the stored (or system) theme onto <html> before React renders and
// keeps following the OS while the choice is "system". The attribute is also
// set by an inline script in index.html before any CSS loads, so the first
// paint is never the wrong theme; this sync owns every change after that.
startThemeSync();

const root = document.getElementById("root");

if (!root) {
  throw new Error("Devboule root element is missing");
}

// The close flow's flush request lands here even when no editor is
// mounted: the listener is process-wide, the registry decides.
void installEditorFlush().catch(() => undefined);

createRoot(root).render(
  <StrictMode>
    <AppRoot />
  </StrictMode>,
);
