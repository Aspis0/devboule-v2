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
 * with the catalog. The value may be the read itself while it is open: a
 * remount during the window (collapse/expand, epoch bump) subscribes to the
 * same promise instead of firing a second probe for the same provider.
 */
export type ModelCountValue = number | null | Promise<number | null>;
export interface ModelCountEntry {
  epoch: number;
  value: ModelCountValue;
}
export type ModelCountCache = Map<string, ModelCountEntry>;

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
  epoch,
}: {
  providerId: string;
  /** True only when the handshake advertised `provider_vocabulary`. */
  supported: boolean;
  cache: ModelCountCache;
  /** Panel generation: entries from another epoch are never trusted. */
  epoch: number;
}) {
  const [count, setCount] = useState<number | null | undefined>(() => {
    const entry = cache.get(providerId);
    if (entry && entry.epoch === epoch && !(entry.value instanceof Promise)) {
      return entry.value;
    }
    return undefined;
  });

  useEffect(() => {
    if (!supported) return;
    let cancelled = false;
    const entry = cache.get(providerId);
    if (entry !== undefined && entry.epoch === epoch) {
      // Settled values are already in state; an open read gets a second
      // subscriber instead of a second probe.
      if (entry.value instanceof Promise) {
        void entry.value.then((value) => {
          if (!cancelled) setCount(value);
        });
      }
      return;
    }
    const pending = providerVocabularyGet(providerId, "", false).then(
      (reply) => {
        const items = reply.models.state === "present" ? (reply.models.items ?? []) : [];
        return items.length > 0 ? items.length : null;
      },
      () => null,
    );
    cache.set(providerId, { epoch, value: pending });
    void pending.then((value) => {
      // The cache is not view state: it is written even when the creating
      // instance unmounted mid-read, so a later remount subscribes instead
      // of seeing a blank frame.
      if (cache.get(providerId)?.value === pending) {
        cache.set(providerId, { epoch, value });
      }
      if (cancelled) return;
      setCount(value);
    });
    return () => {
      cancelled = true;
    };
  }, [providerId, supported, cache, epoch]);

  if (count === undefined || count === null) return null;
  const text = modelCountText(count);
  if (text === null) return null;
  return <span className="prov-model-count"> · {text}</span>;
}
