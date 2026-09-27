import { useEffect, useRef, useState } from "react";
import { toolPolicyGet, toolPolicySet } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import type { ToolPolicyEntry } from "../../../types/ipc";
import { toolPolicyFor } from "../providerStatus";

/** A rejected single-switch write, shown inside its own row. */
export interface ToolWriteError {
  providerId: string;
  error: ErrorSentence;
}

export interface ToolPolicies {
  /** The stored rows; null until the first load lands. */
  policies: readonly ToolPolicyEntry[] | null;
  /** A failed load is terminal until Retry: switches stay locked, never guessed. */
  loadFailed: boolean;
  loadError: ErrorSentence | null;
  retry: () => void;
  setEnabled: (providerId: string, next: boolean) => void;
  turnAllOn: (providerId: string) => void;
  writeError: ToolWriteError | null;
}

/**
 * The panel's single tool-policy store. One fetch answers every row (the old
 * card fetched per provider); every write carries one boolean and an always
 * empty deny list, which is also what normalises a legacy row the first time
 * its switch is touched. Overlap keeps the old card's contract: writes go
 * out in click order, only the newest sequence owns the UI when it settles,
 * and a superseded rejection reverts and reports nothing.
 */
export function useToolPolicies(supported: boolean, active: boolean): ToolPolicies {
  const [policies, setPolicies] = useState<readonly ToolPolicyEntry[] | null>(null);
  const [loadError, setLoadError] = useState<ErrorSentence | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [writeError, setWriteError] = useState<ToolWriteError | null>(null);
  const [loadNonce, setLoadNonce] = useState(0);
  // Synchronous mirror of `policies`: what a second rapid write reads and
  // the base its revert applies to — never the render closure.
  const policiesRef = useRef<readonly ToolPolicyEntry[] | null>(null);
  // Monotonic write sequence: only the newest write owns the UI on settle.
  const seqRef = useRef(0);
  // Writes between sent and settled. The load effect reads this to tell a
  // write that overlapped the fetch (whose reply adopts nothing) apart from
  // one that settled before it started.
  const writesInFlightRef = useRef(0);

  useEffect(() => {
    if (!supported || !active) return;
    let cancelled = false;
    const seqAtFetch = seqRef.current;
    const writeWasInFlight = writesInFlightRef.current > 0;
    void toolPolicyGet()
      .then((reply) => {
        if (cancelled) return;
        if (seqRef.current !== seqAtFetch) return;
        if (writeWasInFlight) return;
        policiesRef.current = reply.policies;
        setPolicies(reply.policies);
        setLoadError(null);
        setLoadFailed(false);
      })
      .catch((cause: unknown) => {
        if (!cancelled) {
          if (policiesRef.current === null) setLoadFailed(true);
          setLoadError(errorSentence(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [supported, active, loadNonce]);

  async function persist(providerId: string, nextEnabled: boolean) {
    const before = policiesRef.current ?? [];
    const hadRow = before.some((entry) => entry.providerId === providerId);
    const previous = toolPolicyFor(providerId, policiesRef.current);
    const seq = ++seqRef.current;
    writesInFlightRef.current += 1;
    setWriteError(null);
    const optimistic: readonly ToolPolicyEntry[] = [
      ...(policiesRef.current ?? []).filter((entry) => entry.providerId !== providerId),
      { providerId, enabled: nextEnabled ? null : false, disabledTools: [] },
    ];
    policiesRef.current = optimistic;
    setPolicies(optimistic);
    try {
      await toolPolicySet(providerId, nextEnabled ? null : false, []);
      return;
    } catch (cause) {
      if (seq !== seqRef.current) return;
      // Put back exactly what this write replaced: the prior row when one
      // existed, nothing at all when the provider never had a row — an
      // (enabled, []) row means the same as absent, but the store should
      // not gain rows a rejection invented.
      const without = (policiesRef.current ?? []).filter(
        (entry) => entry.providerId !== providerId,
      );
      const reverted: readonly ToolPolicyEntry[] = hadRow
        ? [
            ...without,
            {
              providerId,
              enabled: previous.enabled ? null : false,
              disabledTools: [...previous.disabledTools],
            },
          ]
        : without;
      policiesRef.current = reverted;
      setPolicies(reverted);
      setWriteError({ providerId, error: errorSentence(cause) });
    } finally {
      writesInFlightRef.current -= 1;
    }
  }

  function setEnabled(providerId: string, next: boolean) {
    void persist(providerId, next);
  }

  function turnAllOn(providerId: string) {
    void persist(providerId, true);
  }

  function retry() {
    setLoadError(null);
    setLoadFailed(false);
    setLoadNonce((nonce) => nonce + 1);
  }

  return { policies, loadFailed, loadError, retry, setEnabled, turnAllOn, writeError };
}
