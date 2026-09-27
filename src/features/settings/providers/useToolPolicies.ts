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
 * its switch is touched.
 *
 * Sequencing is per provider, never global: each provider has its own write
 * counter and in-flight count, and every fetch captures both at issue time.
 * A rejection is therefore always reported and reverted for its own row,
 * even while another row writes (a global counter would swallow it); a
 * reply issued before a write can never overwrite the newer settled row —
 * for a provider written after the fetch was issued, or with a write still
 * open, the local row stands and the daemon's row is adopted for everyone
 * else, so a reconnect refetch still heals stale rows.
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
  // Write sequence per provider: only the newest write to THAT provider owns
  // its row on settle, so a newer write to another provider never swallows
  // this one's rejection and revert.
  const seqByProvider = useRef(new Map<string, number>());
  // Open writes per provider. A Set would unpin on the first settle while a
  // second overlapping write to the same row is still open; the count keeps
  // the pin until the last one lands.
  const inFlightByProvider = useRef(new Map<string, number>());

  function seqOf(providerId: string): number {
    return seqByProvider.current.get(providerId) ?? 0;
  }

  function inFlightOf(providerId: string): number {
    return inFlightByProvider.current.get(providerId) ?? 0;
  }

  useEffect(() => {
    if (!supported || !active) return;
    let cancelled = false;
    // The reply's own age: a provider written after this fetch was issued —
    // or with a write already open then — keeps its local row, whatever the
    // reply carries. Everyone else adopts the daemon's answer.
    const seqAtFetch = new Map(seqByProvider.current);
    const openAtFetch = new Set(
      [...inFlightByProvider.current.entries()]
        .filter(([, count]) => count > 0)
        .map(([providerId]) => providerId),
    );
    void toolPolicyGet()
      .then((reply) => {
        if (cancelled) return;
        const fetchedById = new Map(reply.policies.map((entry) => [entry.providerId, entry]));
        const currentById = new Map(
          (policiesRef.current ?? []).map((entry) => [entry.providerId, entry]),
        );
        const merged: ToolPolicyEntry[] = [];
        for (const providerId of new Set([...fetchedById.keys(), ...currentById.keys()])) {
          const newerWrite = seqOf(providerId) !== (seqAtFetch.get(providerId) ?? 0);
          const overlapped = openAtFetch.has(providerId) || inFlightOf(providerId) > 0;
          const fetched = fetchedById.get(providerId);
          const current = currentById.get(providerId);
          if ((!newerWrite && !overlapped && fetched !== undefined) || current === undefined) {
            if (fetched !== undefined) merged.push(fetched);
          } else if (current !== undefined) {
            merged.push(current);
          }
        }
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
    seqByProvider.current.set(providerId, seqOf(providerId) + 1);
    const seq = seqOf(providerId);
    inFlightByProvider.current.set(providerId, inFlightOf(providerId) + 1);
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
      // Superseded only by a newer write to THIS provider: another row's
      // traffic never cancels this row's revert and report.
      if (seqOf(providerId) !== seq) return;
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
      const open = inFlightOf(providerId) - 1;
      if (open <= 0) inFlightByProvider.current.delete(providerId);
      else inFlightByProvider.current.set(providerId, open);
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
    // A dead Retry must not linger: when the store cannot fetch (no
    // capability, or no tool-bearing provider left), the alert clears
    // instead of sitting on screen with nothing behind it.
    if (!supported || !active) {
      setLoadError(null);
      setLoadFailed(false);
      return;
    }
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
