/**
 * Tells the shell a modal is up.
 *
 * The shell keeps the crescent from opening over an open modal and the
 * surface from switching under one; a modal that does not say so is one the
 * shell cannot protect. The count, not a flag, so nested modals and
 * overlapping mounts each release their own registration.
 */
import { useEffect } from "react";
import { useAppStore } from "../store/appStore";

export function useModalOpen(isOpen: boolean = true): void {
  useEffect(() => {
    if (!isOpen) return;
    return useAppStore.getState().openModal();
  }, [isOpen]);
}
