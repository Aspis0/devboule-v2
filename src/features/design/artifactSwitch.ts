import { useAppStore } from "../../store/appStore";

/**
 * Puts the artifact one transcript message carries on the canvas. The canvas
 * shows the last card that holds an artifact, so the message moves to the end
 * of the transcript and every other card stays where it is — the switch is the
 * order, never a second record of which card is in front.
 *
 * The move also asks for one fit, so an artifact that arrived while the canvas
 * was showing something else still lands fully in view.
 *
 * Refused while a generation runs: its result takes the end of the transcript
 * when it settles, so the artifact shown would not be the requested one.
 *
 * Returns whether the canvas was pointed at that message.
 */
export function showDesignArtifact(messageId: string): boolean {
  const store = useAppStore.getState();
  const { host, messages, generation } = store.designSession;
  const message = messages.find((candidate) => candidate.id === messageId);
  if (host === null || message === undefined || generation !== null) return false;
  if (messages.at(-1) !== message) {
    store.setDesignMessages(host, (current) => [
      ...current.filter((candidate) => candidate.id !== messageId),
      message,
    ]);
  }
  store.requestMirroredFit(host);
  return true;
}
