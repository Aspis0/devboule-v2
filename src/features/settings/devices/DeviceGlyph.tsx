/**
 * One device glyph for every paired row: a plain monitor drawn in our own
 * strokes, muted like the provider glyph box. A single mark for all rows —
 * a client peer is "a phone or laptop" and a daemon peer "another devboule",
 * so a per-role pictogram would claim a shape the pairing never stated.
 * Hidden from assistive tech; the adjacent name carries the meaning.
 */
export function DeviceGlyph() {
  return (
    <svg className="dev-glyph-mark" viewBox="0 0 24 24" aria-hidden="true">
      <rect x="3" y="4" width="18" height="12" rx="2" />
      <path d="M9 20h6M12 16v4" />
    </svg>
  );
}
