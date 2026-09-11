import { useCallback, useEffect, useRef, useState } from "react";
import type { ProviderInfo } from "../../types/ipc";
import { quotePermissionArg } from "../../components/PermissionCard";

export interface ProviderConsentState {
  pending: ProviderInfo | null;
  request: (provider: ProviderInfo) => void;
  confirm: () => void;
  cancel: () => void;
  inFlight: boolean;
  commandLine: string;
}

/**
 * State machine for consent before an npx-backed provider is launched.
 * Surfaces own their picker markup, focus, and dismissal policy; this hook is
 * deliberately headless so it can be shared by unrelated surfaces.
 */
export function useProviderConsent({
  onConfirmed,
}: {
  onConfirmed: (provider: ProviderInfo) => void;
}): ProviderConsentState {
  const [pending, setPending] = useState<ProviderInfo | null>(null);
  const [inFlight, setInFlight] = useState(false);
  const inFlightRef = useRef(false);

  useEffect(() => {
    // This must run from the pending-provider transition rather than at the
    // end of confirm. A second synchronous confirm sees the old pending value
    // and therefore still has to observe the armed ref guard.
    inFlightRef.current = false;
    setInFlight(false);
  }, [pending]);

  const request = useCallback((provider: ProviderInfo) => {
    if (inFlightRef.current) return;
    setInFlight(false);
    setPending(provider);
  }, []);

  const confirm = useCallback(() => {
    if (pending === null || inFlightRef.current) return;
    inFlightRef.current = true;
    setInFlight(true);
    setPending(null);
    onConfirmed(pending);
  }, [onConfirmed, pending]);

  const cancel = useCallback(() => {
    if (inFlightRef.current) return;
    setPending(null);
  }, []);

  const commandLine =
    pending === null
      ? ""
      : ["npx", "-y", pending.executable, ...(pending.launchArgs ?? [])]
          .map(quotePermissionArg)
          .join(" ");

  return { pending, request, confirm, cancel, inFlight, commandLine };
}
