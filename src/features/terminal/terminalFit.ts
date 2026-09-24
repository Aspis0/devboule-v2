/**
 * The terminal's rows/columns arithmetic, pure so the contract is testable:
 * a content box holds whole cells, and a partial row or column never counts —
 * counting it is exactly the clipped-prompt defect this module exists to keep
 * dead (fix pass 1: the fit counted the host's padding as content).
 */
export interface FitBox {
  width: number;
  height: number;
}

export interface FitGrid {
  cols: number;
  rows: number;
}

/** xterm's own minimums: two columns, one row. */
export function fitRowsCols(box: FitBox, cell: FitBox): FitGrid {
  return {
    cols: Math.max(2, Math.floor(box.width / cell.width)),
    rows: Math.max(1, Math.floor(box.height / cell.height)),
  };
}
