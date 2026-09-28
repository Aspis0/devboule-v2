import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { ConfirmDialog } from "./ConfirmDialog";

/**
 * What a destructive act asks: the title, the body, and the affirmative's
 * label — which names the act (Discard, Delete). The safe answer's label
 * defaults to `Cancel`; the two irreversible acts name it instead (Keep
 * them, Keep it), so the pair states what saying no keeps. Both callers
 * are destructive, so the host always renders the danger tone; no caller
 * picks it.
 */
export interface ConfirmAsk {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
}

/** Resolves true only when a person pressed the affirmative. */
export type ConfirmAskFn = (ask: ConfirmAsk) => Promise<boolean>;

const ConfirmAskContext = createContext<ConfirmAskFn | null>(null);

/** Without a host nothing may reach the wire: the fallback declines. */
function declineWithoutHost(): Promise<boolean> {
  return Promise.resolve(false);
}

export function useConfirmAsk(): ConfirmAskFn {
  return useContext(ConfirmAskContext) ?? declineWithoutHost;
}

/**
 * The one dialog host both destructive surfaces ask through. The provider
 * sits inside the side panel body's key (panel id + workspace id), so a
 * panel or workspace switch unmounts it — and the unmount declines a
 * standing ask, the same as Cancel.
 */
export function ConfirmProvider({ children }: { children: ReactNode }) {
  const [standing, setStanding] = useState<ConfirmAsk | null>(null);
  const resolveRef = useRef<((answer: boolean) => void) | null>(null);
  // Claimed by settle, answered after the close commits: the wire starts
  // once the dialog has unregistered its modal token, so the focus park
  // behind the act never observes the ask's own dialog in the count.
  const pendingRef = useRef<{ resolve: (answer: boolean) => void; answer: boolean } | null>(null);
  const mountedRef = useRef(true);

  useEffect(() => {
    // Set on every mount, not only the first: StrictMode's throwaway mount
    // runs cleanup before the real setup, and an unmounted-forever flag
    // would decline every ask the host is ever given.
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      resolveRef.current?.(false);
      resolveRef.current = null;
      const pending = pendingRef.current;
      pendingRef.current = null;
      // An answered ask keeps its answer: the person already chose, and
      // the close that would have delivered it is gone with the unmount.
      pending?.resolve(pending.answer);
    };
  }, []);

  const ask = useCallback((next: ConfirmAsk): Promise<boolean> => {
    // A second ask while one stands — or is still closing — is declined at
    // once: queuing would hold the wire behind a dialog the person never
    // saw, and hanging is worse.
    if (!mountedRef.current || resolveRef.current !== null || pendingRef.current !== null)
      return Promise.resolve(false);
    return new Promise<boolean>((resolve) => {
      resolveRef.current = resolve;
      setStanding(next);
    });
  }, []);

  const settle = useCallback((answer: boolean): void => {
    const resolve = resolveRef.current;
    if (resolve === null) return;
    resolveRef.current = null;
    pendingRef.current = { resolve, answer };
    setStanding(null);
  }, []);

  // The close — not the settle — ends the ask: the dialog's own effects
  // (the token release, the focus return) are child effects, so they run
  // before this parent one and the answer lands after both.
  useEffect(() => {
    if (standing !== null || pendingRef.current === null) return;
    const pending = pendingRef.current;
    pendingRef.current = null;
    pending.resolve(pending.answer);
  }, [standing]);

  return (
    <ConfirmAskContext.Provider value={ask}>
      {children}
      {/* Always mounted, driven by `open`: unmounting the dialog would skip
          the focus return the dialog owes the trigger, so the close — not
          the unmount — ends the ask, the shape the tab strip already uses. */}
      <ConfirmDialog
        open={standing !== null}
        title={standing?.title ?? ""}
        message={standing?.message ?? ""}
        confirmLabel={standing?.confirmLabel ?? ""}
        cancelLabel={standing?.cancelLabel ?? "Cancel"}
        tone="danger"
        onConfirm={() => settle(true)}
        onCancel={() => settle(false)}
      />
    </ConfirmAskContext.Provider>
  );
}
