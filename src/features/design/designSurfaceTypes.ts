import type { DesignLayer } from "./designHost";
import type { Pan } from "../../lib/canvas/viewportMath";
import type { Project, Workspace } from "../../types/ipc";

export type MessageAction = "stop" | "retry" | "select" | "regenerate";

export interface DesignSnapshot {
  hiddenLayerIds: readonly string[];
  layers: readonly DesignLayer[];
}

export interface DesignViewState {
  // Viewport state is deliberately outside DesignHistory, so undo never moves the camera.
  pan: Pan;
  selectedLayerId: string;
  zoom: number;
}

export interface DesignHistory {
  present: DesignSnapshot;
  past: DesignSnapshot[];
  future: DesignSnapshot[];
  saved: boolean;
}

export interface WorkspaceProject extends Project {
  workspaces: readonly Workspace[];
  workspaceError?: string;
}

/**
 * One line of import feedback. `error` is a file that was not attached, `note`
 * is something the user should know about a file that was — a sanitizer that
 * removed something, or a declared type the bytes contradicted.
 */
export interface AttachmentMessage {
  kind: "error" | "note";
  text: string;
  /**
   * The failing command's raw text behind an `error` line, when one exists.
   * The render keeps it as the line's detail (title plus hidden node), so a
   * mapped sentence never loses the daemon's own words. App-authored lines
   * carry none.
   */
  detail?: string | null;
}

export type SnapshotChange = (current: DesignSnapshot) => DesignSnapshot | null;
