import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  hostOrder,
  readHostRegistry,
  withDiscoveredHosts,
  withHostCollapsed,
  writeHostRegistry,
  type HostRegistry,
  type StorageLike,
} from "./hostRegistry";

export interface HostRegistryView {
  order: readonly string[];
  isCollapsed: (hostId: string) => boolean;
  toggle: (hostId: string) => void;
}

/** Touching `localStorage` throws outright when storage is blocked. */
function defaultStorage(): StorageLike | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

/**
 * The hosts' folds and the order they were first seen in, read once when the
 * surface mounts and written only when the registry really changes — so the
 * poll that repeats the last answer every two seconds costs no write.
 */
export function useHostRegistry(hostIds: readonly string[]): HostRegistryView {
  const [registry, setRegistry] = useState<HostRegistry>(() => readHostRegistry(defaultStorage()));
  // What came out of the store is not a change, so the first render writes nothing.
  const stored = useRef(registry);

  useEffect(() => {
    if (registry === stored.current) return;
    stored.current = registry;
    writeHostRegistry(defaultStorage(), registry);
  }, [registry]);

  useEffect(() => {
    setRegistry((current) => withDiscoveredHosts(current, hostIds));
  }, [hostIds]);

  const order = useMemo(() => hostOrder(registry), [registry]);
  const isCollapsed = useCallback(
    (hostId: string) => registry[hostId]?.collapsed === true,
    [registry],
  );
  const toggle = useCallback((hostId: string) => {
    setRegistry((current) =>
      withHostCollapsed(current, hostId, !(current[hostId]?.collapsed === true)),
    );
  }, []);

  return { order, isCollapsed, toggle };
}
