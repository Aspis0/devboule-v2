import { useEffect, useState } from "react";
import { providerVocabularyGet } from "../../lib/tauri";
import type { ProviderVocabulary } from "../../types/ipc";

/**
 * The provider's own catalog (its models, modes and their efforts) for a Design
 * composer that has no session yet: the same daemon reply the profile form reads,
 * so the first choice is made from what the provider offers. Null while unknown,
 * and whenever a session already exists (its manifest is the answer then).
 */
export function useDesignVocabulary(
  providerId: string | null,
  enabled: boolean,
): ProviderVocabulary | null {
  const [answer, setAnswer] = useState<{
    providerId: string;
    vocabulary: ProviderVocabulary | null;
  } | null>(null);
  useEffect(() => {
    if (!enabled || providerId === null) return;
    let current = true;
    providerVocabularyGet(providerId, "", false)
      .then((vocabulary) => {
        if (current) setAnswer({ providerId, vocabulary: vocabulary ?? null });
      })
      .catch(() => {
        if (current) setAnswer({ providerId, vocabulary: null });
      });
    return () => {
      current = false;
    };
  }, [providerId, enabled]);
  return enabled && providerId !== null && answer?.providerId === providerId
    ? answer.vocabulary
    : null;
}
