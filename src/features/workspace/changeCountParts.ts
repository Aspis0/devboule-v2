import { compactLineCount } from "./sidebar/diffCount";

/** The two sides of a `+N −M` count, each null when it is not drawn. */
export interface CountParts {
  add: string | null;
  del: string | null;
}

/**
 * `+N` and `−M`: a zero side is left out, so a new file reads `+212` and a pure
 * delete `−9`; neither side moved reads `+0`. The minus is U+2212, as the
 * sidebar and the branch total print it.
 */
export function countParts(additions: number, deletions: number): CountParts {
  if (additions === 0 && deletions === 0) return { add: "+0", del: null };
  return {
    add: additions > 0 ? `+${compactLineCount(additions)}` : null,
    del: deletions > 0 ? `−${compactLineCount(deletions)}` : null,
  };
}
