import { useRef, useState } from "react";
import { providerSetEnabled } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import type { ProviderInfo } from "../../../types/ipc";

interface SwitchState {
  enabled: boolean;
  error: ErrorSentence | null;
}

interface FetchSnapshot {
  sequences: Map<string, number>;
  inFlight: Set<string>;
}

/** Optimistic provider switches settle per row and heal from catalog refetches. */
export function useProviderSwitches(supported: boolean) {
  const [states, setStates] = useState<Readonly<Record<string, SwitchState>>>({});
  const statesRef = useRef(states);
  statesRef.current = states;
  const sequences = useRef(new Map<string, number>());
  const inFlight = useRef(new Map<string, number>());

  function isEnabled(provider: ProviderInfo): boolean {
    return statesRef.current[provider.id]?.enabled ?? provider.enabled !== false;
  }

  function beginFetch(): FetchSnapshot {
    return {
      sequences: new Map(sequences.current),
      inFlight: new Set(
        [...inFlight.current.entries()].filter(([, count]) => count > 0).map(([id]) => id),
      ),
    };
  }

  function reconcile(providers: readonly ProviderInfo[], snapshot: FetchSnapshot) {
    const fetched = new Map(providers.map((provider) => [provider.id, provider]));
    const merged: Record<string, SwitchState> = {};
    const ids = new Set([...fetched.keys(), ...Object.keys(statesRef.current)]);
    for (const id of ids) {
      const changedSinceFetch = (sequences.current.get(id) ?? 0) !== (snapshot.sequences.get(id) ?? 0);
      const overlapsWrite = snapshot.inFlight.has(id) || (inFlight.current.get(id) ?? 0) > 0;
      const current = statesRef.current[id];
      const provider = fetched.get(id);
      if (current !== undefined && (changedSinceFetch || overlapsWrite)) {
        merged[id] = current;
      } else if (provider !== undefined) {
        merged[id] = { enabled: provider.enabled !== false, error: null };
      }
    }
    statesRef.current = merged;
    setStates(merged);
  }

  async function setEnabled(provider: ProviderInfo, enabled: boolean) {
    if (!supported) return;
    const id = provider.id;
    const previous = isEnabled(provider);
    const sequence = (sequences.current.get(id) ?? 0) + 1;
    sequences.current.set(id, sequence);
    inFlight.current.set(id, (inFlight.current.get(id) ?? 0) + 1);
    const optimistic = { enabled, error: null };
    statesRef.current = { ...statesRef.current, [id]: optimistic };
    setStates(statesRef.current);
    try {
      await providerSetEnabled(id, enabled);
    } catch (cause) {
      if (sequences.current.get(id) === sequence) {
        const reverted = { enabled: previous, error: errorSentence(cause) };
        statesRef.current = { ...statesRef.current, [id]: reverted };
        setStates(statesRef.current);
      }
    } finally {
      const open = (inFlight.current.get(id) ?? 1) - 1;
      if (open <= 0) inFlight.current.delete(id);
      else inFlight.current.set(id, open);
    }
  }

  return { isEnabled, setEnabled, states, beginFetch, reconcile };
}
