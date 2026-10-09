import { useEffect, useRef, useState } from "react";
import { providersAuthCheck, providersList, providersRefresh } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import type { ProviderCatalog } from "../../../types/ipc";
import type { useProviderSwitches } from "./useProviderSwitches";
import { mergeAuthChecks } from "./providerPanelCopy";

/**
 * The provider catalogue as the Providers page holds it: the mount fetch,
 * the auth check, and Refresh. Install and update results are applied by the
 * consent hook through the setters and `fetchSeqRef` returned here.
 */
export function useProviderCatalog({
  authCheckSupported,
  switches,
  onRefreshStart,
  onRefreshed,
}: {
  authCheckSupported: boolean;
  switches: Pick<ReturnType<typeof useProviderSwitches>, "beginFetch" | "reconcile">;
  /** Runs synchronously when Refresh starts: the page's own lines and counts reset here. */
  onRefreshStart: () => void;
  /** Runs when a Refresh lands. Handoff notes clear only on success, never on a failed refresh. */
  onRefreshed: () => void;
}) {
  const [catalog, setCatalog] = useState<ProviderCatalog | null>(null);
  const [error, setError] = useState<ErrorSentence | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  // Bumped by every fetch (mount and refresh); a response only applies when its
  // sequence is still the latest, so a slow mount list cannot revert a refresh.
  const fetchSeqRef = useRef(0);
  const authCheckDoneEpochRef = useRef(-1);
  // Set synchronously on click so a second click before the re-render is a no-op.
  const refreshInFlightRef = useRef(false);
  const { beginFetch, reconcile } = switches;

  useEffect(() => {
    let cancelled = false;
    const seq = ++fetchSeqRef.current;
    const switchFetch = beginFetch();
    void providersList()
      .then(async (listed) => {
        if (cancelled || seq !== fetchSeqRef.current) return;
        reconcile(listed.providers, switchFetch);
        setCatalog(listed);
      })
      .catch((cause: unknown) => {
        if (!cancelled && seq === fetchSeqRef.current) {
          setCatalog({ providers: [], unreadableDirs: 0 });
          setError(errorSentence(cause));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [beginFetch, reconcile]);

  // The check runs on open and on explicit Refresh only. A mount (or a
  // capability change) bumps this epoch; an install or update never does,
  // so a completed install cannot spawn a silent extra round. The epoch is
  // a boolean dep, not the capabilities array: the 2 s status poll mints a
  // new array identity every tick, and deping on it would spawn a round
  // every two seconds.
  // This effect must run before the check effect below: React runs
  // effects in declaration order, and a check effect that reads the
  // pre-bump epoch fires a second round when the bump re-runs it.
  const authCheckEpochRef = useRef(0);
  useEffect(() => {
    if (authCheckSupported) authCheckEpochRef.current += 1;
  }, [authCheckSupported]);

  useEffect(() => {
    if (!authCheckSupported || catalog === null) return;
    const epoch = authCheckEpochRef.current;
    if (authCheckDoneEpochRef.current === epoch) return;
    // Set eagerly: a check that rejects is never retried for this mount —
    // the row keeps last-start wording until a Refresh or a remount.
    authCheckDoneEpochRef.current = epoch;
    // The fetch sequence at fire time: a result for an older request never
    // overwrites a newer catalog. The daemon's coalescing and reuse window
    // make its observations monotonic per provider, but the client keeps
    // its own guard rather than rely on that.
    const seq = fetchSeqRef.current;
    let cancelled = false;
    void providersAuthCheck(false)
      .then((checked) => {
        if (!cancelled && authCheckEpochRef.current === epoch && seq === fetchSeqRef.current) {
          setCatalog((current) => (current === null ? current : mergeAuthChecks(current, checked)));
        }
      })
      .catch(() => {
        // Catalog discovery still works when an auth check cannot run.
      });
    return () => {
      cancelled = true;
    };
  }, [catalog, authCheckSupported]);

  function refresh() {
    if (refreshInFlightRef.current) return;
    refreshInFlightRef.current = true;
    setRefreshing(true);
    setError(null);
    onRefreshStart();
    const seq = ++fetchSeqRef.current;
    const switchFetch = beginFetch();
    void providersRefresh()
      .then(async (fresh) => {
        let checkedCatalog = fresh;
        try {
          if (authCheckSupported) {
            // A deliberate Refresh measures again: it must never be served
            // the reuse window's possibly-stale observation.
            checkedCatalog = mergeAuthChecks(fresh, await providersAuthCheck(true));
          }
        } catch {
          // Keep the fresh catalog and its last-start fallback.
        }
        if (seq === fetchSeqRef.current) {
          reconcile(checkedCatalog.providers, switchFetch);
          setCatalog(checkedCatalog);
        }
        // The refetch is the proof a handoff landed: installed rows move
        // sections, so waiting notes clear only on success — a Refresh
        // mid-install must not re-offer Install for a run still going.
        onRefreshed();
      })
      .catch((cause: unknown) => {
        if (seq === fetchSeqRef.current) {
          setError(errorSentence(cause));
        }
      })
      .finally(() => {
        // Unconditional on purpose: React 18+ treats setState on an unmounted
        // component as a safe no-op, so the button can never get stuck on
        // "Refreshing…". Do not re-add an unmount guard here.
        refreshInFlightRef.current = false;
        setRefreshing(false);
      });
  }

  return { catalog, setCatalog, error, setError, refreshing, refresh, fetchSeqRef };
}
