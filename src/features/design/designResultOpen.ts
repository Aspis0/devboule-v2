import { useAppStore } from "../../store/appStore";
import { delegatedChildSessionId } from "./delegatedDesignMirror";
import type { DesignPreviewBlockMessage } from "./DesignPreviewBlock";
import { openMirroredDesign } from "./mirroredDesignOpen";

/**
 * Opens the canvas on one result the sidebar's column is previewing.
 *
 * The canvas shows the last result that holds an artifact, so opening a result the
 * column stacks behind another has to move that result to the end of the transcript
 * first — the same move `openMirroredDesign` makes for a mirrored child, which is
 * where a mirrored card goes through its own path instead.
 *
 * Refused while a generation runs, for the mirrored children's reason: that run's
 * result takes the end of the transcript when it settles, so the artifact the canvas
 * opened on would not be the requested one.
 */
export function openDesignResult(message: DesignPreviewBlockMessage): void {
  const childSessionId = delegatedChildSessionId(message.id);
  if (childSessionId !== null) {
    openMirroredDesign(childSessionId);
    return;
  }
  const store = useAppStore.getState();
  const { host, messages, generation } = store.designSession;
  // Re-read the card from the transcript rather than writing the caller's copy: a
  // caller holding an older object must not replace the live message with it.
  const card = messages.find((entry) => entry.id === message.id);
  if (host === null || card === undefined || generation !== null) return;
  if (messages.at(-1) !== card) {
    store.setDesignMessages(host, (current) => [
      ...current.filter((entry) => entry.id !== card.id),
      card,
    ]);
  }
  store.requestMirroredFit(host);
  store.selectSurface("design");
}
