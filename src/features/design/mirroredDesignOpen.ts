import { useAppStore } from "../../store/appStore";
import type { DesignMessage } from "./designHost";
import { delegatedDesignCardId } from "./delegatedDesignMirror";

/** Whether the canvas's transcript holds this child's mirrored card. */
export function hasMirroredDesignCard(
  messages: readonly DesignMessage[],
  childSessionId: string,
): boolean {
  const cardId = delegatedDesignCardId(childSessionId);
  return messages.some((message) => message.id === cardId);
}

/**
 * Opens Design on one mirrored child. The canvas shows the last card that holds
 * an artifact, so the child's card moves to the end of the transcript; the other
 * child's card stays in the list.
 *
 * Refused while a generation runs: its result takes the end of the transcript
 * when it settles, so the artifact shown would not be the requested one.
 */
export function openMirroredDesign(childSessionId: string): void {
  const store = useAppStore.getState();
  const { host, messages, generation } = store.designSession;
  const cardId = delegatedDesignCardId(childSessionId);
  const card = messages.find((message) => message.id === cardId);
  if (host === null || card === undefined || generation !== null) return;
  if (messages.at(-1) !== card) {
    store.setDesignMessages(host, (current) => [
      ...current.filter((message) => message.id !== cardId),
      card,
    ]);
  }
  store.selectSurface("design");
}
