import { createContext, useContext, useState, useSyncExternalStore } from "react";

/**
 * Which tool lines the person has opened, by row id. A line keeps its open
 * state past the component that draws it: a lone call that becomes the first
 * call of a group is drawn by a new component, and its open must come with it.
 */
export interface OpenedRows {
  isOpen: (id: string) => boolean;
  setOpen: (id: string, open: boolean) => void;
  subscribe: (listener: () => void) => () => void;
}

export function createOpenedRows(): OpenedRows {
  const open = new Set<string>();
  const listeners = new Set<() => void>();
  return {
    isOpen: (id) => open.has(id),
    setOpen(id, value) {
      if (open.has(id) === value) return;
      if (value) open.add(id);
      else open.delete(id);
      for (const listener of listeners) listener();
    },
    subscribe(listener) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

/** The transcript's own store; a line outside a transcript keeps its own. */
export const OpenedRowsContext = createContext<OpenedRows | null>(null);

/** A line's open state and its setter. Only the line whose state changed re-renders. */
export function useOpenedRow(id: string): [boolean, (open: boolean) => void] {
  const shared = useContext(OpenedRowsContext);
  const [ownStore] = useState(createOpenedRows);
  const store = shared ?? ownStore;
  const open = useSyncExternalStore(store.subscribe, () => store.isOpen(id));
  return [open, (value) => store.setOpen(id, value)];
}
