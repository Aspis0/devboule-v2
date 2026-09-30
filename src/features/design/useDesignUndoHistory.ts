import { useCallback, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import type { DesignHistory, DesignSnapshot, SnapshotChange } from "./designSurfaceTypes";
import type { DesignLayer } from "./designHost";

interface UseDesignUndoHistoryInput {
  initialSnapshot: DesignSnapshot;
  initialSaved: boolean;
  surfaceRef: RefObject<HTMLElement | null>;
}

interface UseDesignUndoHistoryResult {
  snapshot: DesignSnapshot;
  layers: readonly DesignLayer[];
  saved: boolean;
  canUndo: boolean;
  canRedo: boolean;
  revisionRef: RefObject<number>;
  markDocumentDirty: () => void;
  toggleLayerVisibility: (layerId: string) => void;
  undo: () => void;
  redo: () => void;
  setSaved: (saved: boolean) => void;
}

export function useDesignUndoHistory(input: UseDesignUndoHistoryInput): UseDesignUndoHistoryResult {
  const [history, setHistory] = useState<DesignHistory>(() => ({
    present: input.initialSnapshot,
    past: [],
    future: [],
    saved: input.initialSaved,
  }));
  const revisionRef = useRef(0);

  const snapshot = history.present;
  const layers = snapshot.layers;
  const saved = history.saved;
  const canUndo = history.past.length > 0;
  const canRedo = history.future.length > 0;

  const commitSnapshot = useCallback((change: SnapshotChange) => {
    revisionRef.current += 1;
    setHistory((current) => {
      const next = change(current.present);
      if (next === null) return current;
      return {
        ...current,
        present: next,
        past: [...current.past, current.present],
        future: [],
        saved: false,
      };
    });
  }, []);

  const markDocumentDirty = useCallback(() => {
    revisionRef.current += 1;
    setHistory((current) => (current.saved ? { ...current, saved: false } : current));
  }, []);

  const toggleLayerVisibility = useCallback(
    (layerId: string) => {
      commitSnapshot((current) => ({
        ...current,
        hiddenLayerIds: current.hiddenLayerIds.includes(layerId)
          ? current.hiddenLayerIds.filter((id) => id !== layerId)
          : [...current.hiddenLayerIds, layerId],
      }));
    },
    [commitSnapshot],
  );

  const undo = useCallback(() => {
    if (!canUndo) return;
    revisionRef.current += 1;
    setHistory((current) => {
      if (current.past.length === 0) return current;
      const previous = current.past[current.past.length - 1];
      return {
        ...current,
        present: previous,
        past: current.past.slice(0, -1),
        future: [current.present, ...current.future],
        saved: false,
      };
    });
  }, [canUndo]);

  const redo = useCallback(() => {
    if (!canRedo) return;
    revisionRef.current += 1;
    setHistory((current) => {
      if (current.future.length === 0) return current;
      const next = current.future[0];
      return {
        ...current,
        present: next,
        past: [...current.past, current.present],
        future: current.future.slice(1),
        saved: false,
      };
    });
  }, [canRedo]);

  useEffect(() => {
    const surface = input.surfaceRef.current;
    if (!surface) return;

    const handleKeyDown = (event: globalThis.KeyboardEvent) => {
      if (!event.ctrlKey || event.altKey || event.key.toLowerCase() !== "z") return;

      const target = event.target;
      if (
        target instanceof HTMLInputElement ||
        target instanceof HTMLTextAreaElement ||
        (target instanceof HTMLElement && target.isContentEditable)
      ) {
        return;
      }

      if (event.shiftKey) {
        if (!canRedo) return;
        event.preventDefault();
        redo();
        return;
      }

      if (!canUndo) return;
      event.preventDefault();
      undo();
    };

    surface.addEventListener("keydown", handleKeyDown);
    return () => surface.removeEventListener("keydown", handleKeyDown);
  }, [canRedo, canUndo, input.surfaceRef, redo, undo]);

  const setSaved = useCallback((saved: boolean) => {
    setHistory((current) => (current.saved === saved ? current : { ...current, saved }));
  }, []);

  return {
    snapshot,
    layers,
    saved,
    canUndo,
    canRedo,
    revisionRef,
    markDocumentDirty,
    toggleLayerVisibility,
    undo,
    redo,
    setSaved,
  };
}
