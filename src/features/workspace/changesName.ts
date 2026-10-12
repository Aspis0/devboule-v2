/** The characters a row keeps at the end of a name when it cuts the middle: the extension and three more. */
const KEPT_TAIL = 6;

/**
 * A file name split for the row's cut: the head yields to an ellipsis, the tail
 * stays whole. Names that share a long prefix (`CODER-REPORT-…`) still read
 * apart at their end, where the extension and the distinguishing part sit.
 */
export function splitFileName(name: string): { head: string; tail: string } {
  const chars = Array.from(name);
  if (chars.length <= KEPT_TAIL) return { head: "", tail: name };
  return {
    head: chars.slice(0, -KEPT_TAIL).join(""),
    tail: chars.slice(-KEPT_TAIL).join(""),
  };
}
