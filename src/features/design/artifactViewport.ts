/**
 * The canonical viewport a generated page is authored against and rendered at.
 *
 * A generated page has no intrinsic width: the platform has to choose one. 1280
 * CSS px is the standard desktop browser viewport and leaves room for the
 * 1080 px max-width shells the design doctrine targets. The 800 px height is the
 * desktop browser content viewport for that width.
 *
 * These are one source of truth so the on-canvas frame and the off-screen render
 * check measure the page at the same width instead of at two different guesses.
 * The width pact is load-bearing: keep ARTIFACT_PAGE_WIDTH identical for both.
 * The height below is adaptive on purpose (see artifactPageHeightForCanvas) and
 * the render critic intentionally stays at ARTIFACT_PAGE_HEIGHT, because its
 * four checks (contrast, pointer-target size, horizontal overflow, focus
 * indicators) are width-driven and height-independent, and re-running its
 * ~1.5 s measurement on every resize would flicker verdicts without new signal.
 */
export const ARTIFACT_PAGE_WIDTH = 1280;
export const ARTIFACT_PAGE_HEIGHT = 800;

/**
 * Adaptive frame bounds. Width stays 1280 (the layout viewport the page is
 * authored against; changing it would move media queries and columns). Height
 * is only "how much page is visible before scrolling", so matching it to the
 * canvas aspect fills the canvas without falsifying the layout.
 */
export const ARTIFACT_PAGE_MIN_HEIGHT = 800;
export const ARTIFACT_PAGE_MAX_HEIGHT = 2000;

/**
 * Minimum change in canvas aspect ratio (height / width) that reframes the
 * artifact. 0.02 is ~18 CSS px of height on an ~898 px wide canvas, so a
 * one-pixel drag or a scrollbar appearing does not re-render the iframe (which
 * would flicker the page), while a real window reshape does.
 */
export const ARTIFACT_CANVAS_RATIO_DELTA = 0.02;

/** Canvas aspect ratio (height / width), or null when the size is unusable. */
export function canvasAspectRatio(width: number, height: number): number | null {
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
    return null;
  }
  return height / width;
}

/**
 * Frame height that fills a canvas of the given live size at the fixed 1280
 * page width: round(1280 * canvasHeight / canvasWidth), clamped to
 * [ARTIFACT_PAGE_MIN_HEIGHT, ARTIFACT_PAGE_MAX_HEIGHT] so a very short or very
 * tall window never produces an absurd sheet. Falls back to
 * ARTIFACT_PAGE_HEIGHT for an unusable size.
 */
export function artifactPageHeightForCanvas(canvasWidth: number, canvasHeight: number): number {
  const ratio = canvasAspectRatio(canvasWidth, canvasHeight);
  if (ratio === null) return ARTIFACT_PAGE_HEIGHT;
  const raw = Math.round(ARTIFACT_PAGE_WIDTH * ratio);
  return Math.min(ARTIFACT_PAGE_MAX_HEIGHT, Math.max(ARTIFACT_PAGE_MIN_HEIGHT, raw));
}

/**
 * True when the canvas moved enough to reframe without flickering on every
 * pixel: either there was no usable previous size and the next one is usable,
 * or the aspect ratio moved by more than ARTIFACT_CANVAS_RATIO_DELTA.
 * A next size that is unusable never reframes.
 */
export function shouldAdaptArtifactHeight(
  prevWidth: number,
  prevHeight: number,
  nextWidth: number,
  nextHeight: number,
): boolean {
  const nextRatio = canvasAspectRatio(nextWidth, nextHeight);
  if (nextRatio === null) return false;
  const prevRatio = canvasAspectRatio(prevWidth, prevHeight);
  if (prevRatio === null) return true;
  return Math.abs(nextRatio - prevRatio) > ARTIFACT_CANVAS_RATIO_DELTA;
}
