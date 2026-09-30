import { nodesBounds } from "../../lib/canvas/viewportMath";
import type { DesignLayer } from "./designHost";
import type { NodeRect, Point } from "../../types/geometry";
import { ARTIFACT_PAGE_WIDTH } from "./artifactViewport";

const DESIGN_GRID_ORIGIN_X = 60;

const DESIGN_GRID_ORIGIN_Y = 46;

// The generated page is a desktop page: it is authored against the canonical
// 1280px page width (see artifactViewport). Width stays fixed so media queries
// and columns do not move; height follows the live canvas aspect so the fitted
// page fills the canvas instead of letterboxing below it.
const ARTIFACT_NODE_WIDTH = ARTIFACT_PAGE_WIDTH;

const ARTIFACT_NODE_GAP = 32;

export const ARTIFACT_NODE_ID = "generated-artifact";

export const ARTIFACT_CONTEXT_NAME = "Generated artifact";

// A gutter, not a frame. The generated page is authored at 1280px and the
// canvas next to a 366px assistant column is under 900px, so every pixel of
// margin is a pixel the page does not get: at 80 per side the page fitted at
// 58% of its true size on a canvas that had room for 70%.
export const DESIGN_FIT_MARGIN = 24;

export function layerRectsFor(layers: readonly DesignLayer[]): NodeRect[] {
  return layers.map((layer, index) => ({
    id: layer.id,
    x: layer.transform.x,
    y: layer.transform.y,
    w: layer.transform.width,
    h: layer.transform.height,
    z: index,
  }));
}

export function artifactNodeRect(layers: readonly DesignLayer[], height: number): NodeRect {
  // The artifact owns its origin: only canvas layers (TSX/SVG) push it down.
  // Section layers live INSIDE its frame, so they are excluded here — feeding
  // them back in would make the frame depend on the sections that depend on
  // the frame. With no canvas layers the artifact sits at the grid origin.
  const bounds = nodesBounds(layerRectsFor(layers.filter((layer) => layer.kind !== "SECTION")));
  return {
    id: ARTIFACT_NODE_ID,
    x: bounds?.x ?? DESIGN_GRID_ORIGIN_X,
    y: bounds === null ? DESIGN_GRID_ORIGIN_Y : bounds.y + bounds.h + ARTIFACT_NODE_GAP,
    w: ARTIFACT_NODE_WIDTH,
    h: height,
    // Canvas nodes only: sections are measured inside the frame, not placed on it.
    z: layers.filter((layer) => layer.kind !== "SECTION").length,
  };
}

/**
 * Direct-on-canvas section pick. Page sections nest (a `nav` inside a
 * `header` inside the body), so several rects contain the pointer at once.
 * Rule: the SMALLEST area containing the point wins — the deepest element is
 * the one the pointer is on. The overlay buttons below are painted
 * largest-first so the smallest is on top, and the canvas click path checks
 * sections with this same helper first: both paths pick the same id, and both
 * call the shared `onSelectLayer`, so canvas selection and panel selection
 * are one state, not two.
 *
 * The point and the rects must describe the same picture. The artifact window
 * scrolls its page, so the caller passes section layers whose tops already
 * carry that offset (see `stageSectionTop`); handing over the measured
 * page-space rects here while the pointer is in stage space is what made a
 * click on a scrolled page pick the section that would be under it at the top.
 */
export function smallestSectionAt(
  sections: readonly DesignLayer[],
  hiddenLayerIds: readonly string[],
  point: Point,
): DesignLayer | null {
  let best: DesignLayer | null = null;
  let bestArea = Number.POSITIVE_INFINITY;
  for (const section of sections) {
    if (hiddenLayerIds.includes(section.id)) continue;
    const box = section.transform;
    if (
      point.x < box.x ||
      point.x > box.x + box.width ||
      point.y < box.y ||
      point.y > box.y + box.height
    ) {
      continue;
    }
    const area = box.width * box.height;
    if (area < bestArea) {
      best = section;
      bestArea = area;
    }
  }
  return best;
}
