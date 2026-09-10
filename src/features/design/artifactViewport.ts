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
 */
export const ARTIFACT_PAGE_WIDTH = 1280;
export const ARTIFACT_PAGE_HEIGHT = 800;
