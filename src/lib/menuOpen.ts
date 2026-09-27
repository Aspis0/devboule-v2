/**
 * Tells the shell a menu is up, so the shell can dismiss it the way every
 * menu is already dismissed: the band opening is the outside press. A picker
 * left standing while the page slides out from under it is a menu the user
 * can no longer see, and a nav click would unmount it with the user's
 * choice still in it.
 */
import { useEffect, useLayoutEffect, useRef } from "react";

const closeHandlers = new Set<() => void>();

export function useMenuOpen(isOpen: boolean, onClose: () => void): void {
  const onCloseRef = useRef(onClose);
  useLayoutEffect(() => {
    onCloseRef.current = onClose;
  });
  useEffect(() => {
    if (!isOpen) return;
    const dismiss = () => onCloseRef.current();
    closeHandlers.add(dismiss);
    return () => {
      closeHandlers.delete(dismiss);
    };
  }, [isOpen]);
}

export function closeOpenMenus(): void {
  for (const dismiss of [...closeHandlers]) dismiss();
}
