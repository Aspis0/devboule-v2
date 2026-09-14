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
