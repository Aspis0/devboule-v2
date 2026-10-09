// Why: a drop on the tab row needs each chip's box to know where the pointer
// falls among them. The chip's id is the only link back to its tab, and it is
// read through the same prefix that names the chip, so the two cannot drift.

import type { TabSlot } from "./tabOrder";
import { sessionTabElementId } from "./useTabCloseFlow";

const CHIP_ID_PREFIX = sessionTabElementId("");

export function readTabSlots(strip: ParentNode): TabSlot[] {
  return [...strip.querySelectorAll<HTMLElement>(".workspace-session-tab")].map((chip) => {
    const box = chip.getBoundingClientRect();
    return {
      id: chip.id.slice(CHIP_ID_PREFIX.length),
      left: box.left,
      right: box.right,
    };
  });
}
