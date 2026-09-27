/**
 * Tells the shell a modal is up.
 *
 * The shell keeps the crescent from opening over an open modal and the
 * surface from switching under one; a modal that does not say so is one the
 * shell cannot protect. Registration is a token in a set, not a counter: a
 * release only ever removes its own token, so a double release, a
 * StrictMode double effect or an unmount without a close cannot leave the
 * band shut. `isOpen` is required — a modal that is mounted while closed
 * must say so with the boolean, never by default.
 */
import { useEffect } from "react";
import { useAppStore } from "../store/appStore";

export function useModalOpen(isOpen: boolean): void {
  useEffect(() => {
    if (!isOpen) return;
    return useAppStore.getState().openModal();
  }, [isOpen]);
}
