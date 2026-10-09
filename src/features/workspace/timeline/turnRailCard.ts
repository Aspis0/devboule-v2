// Why: the card is centred on its dot, so a dot near the top or bottom of the
// visible transcript would put half of the card outside the scrollport, where
// it is clipped. This works out how far the card has to move to stay inside.

/** The gap kept between the card and the visible edge of the transcript. */
const EDGE_PAD_PX = 8;

export interface CardFrame {
  /** The dot's centre, in the same axis as the bounds. */
  dotCenter: number;
  cardHeight: number;
  /** The visible transcript's top and bottom edges. */
  top: number;
  bottom: number;
}

/** The px the card moves from being centred on its dot. A card taller than the
 * transcript keeps its top on the edge rather than its middle. */
export function previewCardShift({ dotCenter, cardHeight, top, bottom }: CardFrame): number {
  const centred = dotCenter - cardHeight / 2;
  const lowest = bottom - EDGE_PAD_PX - cardHeight;
  const highest = top + EDGE_PAD_PX;
  const placed = Math.max(highest, Math.min(centred, lowest));
  return placed - centred;
}
