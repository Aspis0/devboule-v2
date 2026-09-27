import { useRef, useState } from "react";
import { providerSetEnabled } from "../../../lib/tauri";
import { errorSentence, type ErrorSentence } from "../../../lib/errorSentence";
import type { ProviderInfo } from "../../../types/ipc";

interface SwitchState {
  enabled: boolean;
  error: ErrorSentence | null;
}

/** Optimistic provider switches settle and revert independently per row. */
export function useProviderSwitches() {
  const [states, setStates] = useState<Readonly<Record<string, SwitchState>>>({});
  const statesRef = useRef(states);
  statesRef.current = states;
  const sequences = useRef(new Map<string, number>());

  function isEnabled(provider: ProviderInfo): boolean {
    return statesRef.current[provider.id]?.enabled ?? provider.enabled !== false;
  }

  async function setEnabled(provider: ProviderInfo, enabled: boolean) {
    const id = provider.id;
    const previous = isEnabled(provider);
    const sequence = (sequences.current.get(id) ?? 0) + 1;
    sequences.current.set(id, sequence);
    const optimistic = { enabled, error: null };
    statesRef.current = { ...statesRef.current, [id]: optimistic };
    setStates(statesRef.current);
    try {
      await providerSetEnabled(id, enabled);
    } catch (cause) {
      if (sequences.current.get(id) !== sequence) return;
      const reverted = { enabled: previous, error: errorSentence(cause) };
      statesRef.current = { ...statesRef.current, [id]: reverted };
      setStates(statesRef.current);
    }
  }

  return { isEnabled, setEnabled, states };
}
