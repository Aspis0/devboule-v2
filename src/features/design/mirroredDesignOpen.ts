import { useAppStore } from "../../store/appStore";
import type { DesignMessage } from "./designHost";
import { delegatedDesignCardId } from "./delegatedDesignMirror";
import { showDesignArtifact } from "./artifactSwitch";

/** Whether the canvas's transcript holds this child's mirrored card. */
export function hasMirroredDesignCard(
  messages: readonly DesignMessage[],
  childSessionId: string,
): boolean {
  const cardId = delegatedDesignCardId(childSessionId);
  return messages.some((message) => message.id === cardId);
}

/**
 * Opens Design on one mirrored child. The switch itself is `showDesignArtifact`
 * — the same move-to-end the canvas toolbar's switcher uses — and all this adds
 * is the surface change, which a switch from inside the canvas has no reason to
 * make. A refused switch leaves the surface alone.
 */
export function openMirroredDesign(childSessionId: string): void {
  if (!showDesignArtifact(delegatedDesignCardId(childSessionId))) return;
  useAppStore.getState().selectSurface("design");
}
