import { useEffect, useRef, useState } from "react";
import { toolPolicyGet, toolPolicySet } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import type { ToolPolicyEntry } from "../../../types/ipc";
import { toolPolicyFor } from "../providerStatus";

export interface ToolPolicies {
  /** The stored rows; null until the first load lands. */
  policies: readonly ToolPolicyEntry[] | null;
  /** A failed load is terminal until Retry: switches stay locked, never guessed. */
  loadFailed: boolean;
  loadError: ErrorSentence | null;
  retry: () => void;
  setEnabled: (providerId: string, next: boolean) => void;
  turnAllOn: (providerId: string) => void;
  /** Rejected writes by provider: one row's failure never erases another's. */
  writeErrors: Readonly<Record<string, ErrorSentence>>;
  dismissWriteError: (providerId: string) => void;
}

/**
 * The panel's single tool-policy store. One fetch answers every row (the old
 * card fetched per provider); every write carries one boolean and an always
 * empty deny list, which is also what normalises a legacy row the first time
 * its switch is touched. Overlap keeps the old card's contract: writes go
 * out in click order and only the newest sequence owns the UI when it
 * settles, while a fetch reply merges around in-flight writes instead of
 * dropping them — so a reconnect refetch heals stale rows without clobbering
 * an optimistic one.
 */
export function useToolPolicies(supported: boolean, active: boolean): ToolPolicies {
  const [policies, setPolicies] = useState<readonly ToolPolicyEntry[] | null>(null);
  const [loadError, setLoadError] = useState<ErrorSentence | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [writeErrors, setWriteErrors] = useState<Readonly<Record<string, ErrorSentence>>>({});
  const [loadNonce, setLoadNonce] = useState(0);
  // Synchronous mirror of `policies`: what a second rapid write reads and
  // the base its revert applies to — never the render closure.
  const policiesRef = useRef<readonly ToolPolicyEntry[] | null>(null);
  // Monotonic write sequence: only the newest write owns the UI on settle.
  const seqRef = useRef(0);
  // Providers with a write between sent and settled. A fetch reply adopts
  // the daemon's rows for everyone else and keeps these rows' optimistic
  // state; each write's own settle confirms or reverts it.
  const pinnedRef = useRef<Set<string>>(new Set());

  useEffect(() => {
    if (!supported || !active) return;
    let cancelled = false;
    void toolPolicyGet()
      .then((reply) => {
        if (cancelled) return;
        const pinned = pinnedRef.current;
        const kept = (policiesRef.current ?? []).filter((entry) => pinned.has(entry.providerId));
        const fetched = reply.policies.filter((entry) => !pinned.has(entry.providerId));
        const merged = [...fetched, ...kept];
        policiesRef.current = merged;
        setPolicies(merged);
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
    pinnedRef.current.add(providerId);
    const seq = ++seqRef.current;
    // A new write to this provider retires its own unacknowledged failure;
    // other providers' reports stand until theirs is touched or dismissed.
    setWriteErrors((errors) => {
      if (!(providerId in errors)) return errors;
      const next = { ...errors };
      delete next[providerId];
      return next;
    });
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
      // existed, nothing at all when the provider never had a row.
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
      const failure = errorSentence(cause);
      setWriteErrors((errors) => ({ ...errors, [providerId]: failure }));
    } finally {
      pinnedRef.current.delete(providerId);
    }
  }

  function setEnabled(providerId: string, next: boolean) {
    void persist(providerId, next);
  }

  function turnAllOn(providerId: string) {
    void persist(providerId, true);
  }

  function dismissWriteError(providerId: string) {
    setWriteErrors((errors) => {
      if (!(providerId in errors)) return errors;
      const next = { ...errors };
      delete next[providerId];
      return next;
    });
  }

  function retry() {
    setLoadError(null);
    setLoadFailed(false);
    setLoadNonce((nonce) => nonce + 1);
  }

  return {
    policies,
    loadFailed,
    loadError,
    retry,
    setEnabled,
    turnAllOn,
    writeErrors,
    dismissWriteError,
  };
}
