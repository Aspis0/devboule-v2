import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { editorsFlushed } from "../../../lib/tauri";
import type { FileEditorModel } from "./model";

/** A live editor the close flow may ask to flush: the model plus the
 * label the quit question names it by (the path as opened). */
interface RegisteredEditor {
  model: FileEditorModel;
  label: string;
}

let nextId = 1;
const editors = new Map<number, RegisteredEditor>();

/** Track a mounted editor's model until it unmounts. Returns the
 * release; the hook calls it in the same cleanup that disposes. */
export function registerEditor(model: FileEditorModel, label: string): () => void {
  const id = nextId++;
  editors.set(id, { model, label });
  return () => {
    editors.delete(id);
  };
}

/** Labels of editors holding user text the disk does not have: dirty,
 * failed, or conflicted-with-edits. A clean conflict (disk moved, no
 * local edits) is not unsaved — there is nothing of the user's to lose. */
export function unsavedEditorLabels(): string[] {
  const labels: string[] = [];
  for (const { model, label } of editors.values()) {
    const snapshot = model.getSnapshot();
    if (
      snapshot.status === "dirty" ||
      snapshot.status === "error" ||
      (snapshot.status === "conflict" && snapshot.modified)
    ) {
      labels.push(label);
    }
  }
  return labels;
}

/** Save every dirty or failed editor and report what is still unsaved:
 * conflicted-with-edits (no automatic write is safe there) and saves
 * that did not land in time. Bounded by `timeoutMs` — a host that never
 * answers must not hold the close past it. */
export async function flushEditors(timeoutMs: number): Promise<string[]> {
  const saves: Array<Promise<unknown>> = [];
  for (const { model } of editors.values()) {
    const status = model.getSnapshot().status;
    if (status === "dirty" || status === "error") saves.push(model.save().catch(() => undefined));
  }
  await Promise.race([
    Promise.allSettled(saves),
    new Promise((resolve) => setTimeout(resolve, timeoutMs)),
  ]);
  return unsavedEditorLabels();
}

/** The event the close flow emits before acting: flush (3 s bound) and
 * answer with what is still unsaved, so the quit question can name it.
 * Installed once at startup. */
export function installEditorFlush(): Promise<UnlistenFn> {
  return listen<{ nonce: number }>("devboule:flush-editors", (event) => {
    void (async () => {
      const unsaved = await flushEditors(3000);
      try {
        await editorsFlushed(event.payload.nonce, unsaved);
      } catch {
        // The answer road is gone (the app is already leaving): the saves
        // above already went out, which is all a flush can promise.
      }
    })();
  });
}
