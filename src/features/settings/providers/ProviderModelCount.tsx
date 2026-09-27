import { useEffect, useState } from "react";
import { providerVocabularyGet } from "../../../lib/tauri";
import { modelCountText } from "../providerStatus";

/**
 * The handshake capability gating the vocabulary read. Spelled exactly like
 * the daemon's own name; a daemon that does not advertise it would refuse
 * the request, so it is never sent there.
 */
export const PROVIDER_VOCABULARY_CAPABILITY = "provider_vocabulary";

/**
 * Panel-owned model-count cache: provider id to its counted models, or null
 * when the probe said nothing countable. Owned by the panel (not the module)
 * so a remount starts unpoisoned, and cleared on Refresh so counts revalidate
 * with the catalog.
 */
export type ModelCountCache = Map<string, number | null>;

/**
 * The "N models" suffix of one expanded row. Mounts only with the expanded
 * details, so the panel never fires N probes on mount — an ACP cold read
 * starts the provider process. One cache-friendly read per provider
 * (`refresh: false` lets the daemon answer from cache); `absent`, `none`,
 * an empty list, or a failed read all render as no suffix, never "0 models"
 * and never an error: the count is garnish on the status word, not a load.
 */
export function ProviderModelCount({
  providerId,
  supported,
  cache,
}: {
  providerId: string;
  /** True only when the handshake advertised `provider_vocabulary`. */
  supported: boolean;
  cache: ModelCountCache;
}) {
  const [count, setCount] = useState<number | null | undefined>(() =>
    cache.has(providerId) ? (cache.get(providerId) ?? null) : undefined,
  );

  useEffect(() => {
    if (!supported || cache.has(providerId)) return;
    let cancelled = false;
    void providerVocabularyGet(providerId, "", false).then(
      (reply) => {
        if (cancelled) return;
        const items = reply.models.state === "present" ? (reply.models.items ?? []) : [];
        const next = items.length > 0 ? items.length : null;
        cache.set(providerId, next);
        setCount(next);
      },
      () => {
        if (cancelled) return;
        cache.set(providerId, null);
        setCount(null);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [providerId, supported, cache]);

  if (count === undefined || count === null) return null;
  const text = modelCountText(count);
  if (text === null) return null;
  return <span className="prov-model-count"> · {text}</span>;
}
