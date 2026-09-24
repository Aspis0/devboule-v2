/**
 * Bounds text by grapheme clusters, never UTF-16 code units.
 *
 * A unit-based cut splits an astral scalar in half and renders U+FFFD
 * (re-audit F12), and a unit-based pre-check appends an ellipsis to a value
 * whose scalar count was already inside the bound — claiming a truncation
 * that did not happen. Clusters rather than bare scalars, so a base character
 * never loses its combining mark either. `Intl.Segmenter` is the cluster
 * source where the host has one; the scalar fallback keeps the astral
 * guarantees on hosts that do not.
 */
export function boundByGraphemes(value: string, limit: number): string {
  const Segmenter = Intl.Segmenter;
  const clusters =
    typeof Segmenter === "function"
      ? Array.from(
          new Segmenter("en", { granularity: "grapheme" }).segment(value),
          (part) => part.segment,
        )
      : Array.from(value);
  if (clusters.length <= limit) return value;
  return `${clusters.slice(0, limit).join("")}…`;
}

/**
 * The first grapheme cluster of a value, with no ellipsis — an avatar letter,
 * never a truncation. Where `Intl.Segmenter` is missing, the fallback keeps
 * the first code point together with the combining marks that follow it, so a
 * decomposed letter (e + U+0301) survives whole; a host without Segmenter and
 * without decomposed names loses only emoji-sequence fidelity, never a base
 * letter.
 */
export function firstGrapheme(value: string): string {
  const trimmed = value.trim();
  if (trimmed === "") return "";
  const Segmenter = Intl.Segmenter;
  if (typeof Segmenter === "function") {
    const first = new Segmenter("en", { granularity: "grapheme" })
      .segment(trimmed)
      [Symbol.iterator]()
      .next();
    return first.done ? "" : first.value.segment;
  }
  const points = Array.from(trimmed);
  const marks = /^.[\u0300-\u036f\u1ab0-\u1aff\u1dc0-\u1dff\u20d0-\u20f0\ufe20-\ufe2f]*/u.exec(
    points.join(""),
  );
  return marks === null ? points[0]! : marks[0];
}
