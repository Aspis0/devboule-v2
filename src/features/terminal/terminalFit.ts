/**
 * The terminal's rows/columns arithmetic, pure so the contract is testable:
 * a content box holds whole cells, and a partial row or column never counts —
 * counting it clips the prompt, because the fit counts the host's padding as content.
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
