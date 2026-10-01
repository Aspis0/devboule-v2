import type { FitGrid } from "./terminalFit";

// A new terminal is created before any view exists to measure it, so the last
// grid a laid-out terminal fitted stands in for the pane it will open in.
let lastGrid: FitGrid | null = null;

export function recordFittedGrid(grid: FitGrid): void {
  lastGrid = { cols: grid.cols, rows: grid.rows };
}

export function lastFittedGrid(): FitGrid | null {
  return lastGrid;
}
