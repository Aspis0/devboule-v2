// Ported from Paseo's `packages/app/src/file-pane/editor/view.web.tsx`
// (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra): the CodeMirror 6
// host — one `EditorView` per mount, local keystrokes forwarded to the
// model with the model's line separator rejoined, remote content applied
// without touching history, and an optional line target scrolled into
// view. No vim mode and no find panel (both out of scope for this slice);
// the theme comes from this app's tokens through `readEditorTheme`.
//
// Paseo source: `packages/app/src/file-pane/editor/view.web.tsx`.

import { useEffect, useRef, useSyncExternalStore } from "react";
import { Annotation, Compartment, EditorState, Transaction } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { FileEditorModel } from "./model";
import { editorBaseExtensions, editorTheme, readEditorTheme } from "./extensions";

interface FileEditorViewProps {
  model: FileEditorModel;
  filename: string;
  /** 1-based line to land on when the caller names one; null is no jump. */
  lineStart: number | null;
  lineEnd: number | null;
  /** Bumped when the same line is asked for again: re-scrolls. */
  navigationRevision: number;
  onCursorChange(position: { line: number; column: number }): void;
}

const themeCompartment = new Compartment();

export function FileEditorView({
  model,
  filename,
  lineStart,
  lineEnd,
  navigationRevision,
  onCursorChange,
}: FileEditorViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const snapshot = useSyncExternalStore(model.subscribe, model.getSnapshot, model.getSnapshot);
  const initial = useRef({ model, content: snapshot.content });
  const onCursorChangeRef = useRef(onCursorChange);
  onCursorChangeRef.current = onCursorChange;

  useEffect(() => {
    if (!hostRef.current) return;
    const values = initial.current;
    const view = new EditorView({
      parent: hostRef.current,
      state: EditorState.create({
        doc: values.content,
        extensions: [
          ...editorBaseExtensions(() => void values.model.save()),
          themeCompartment.of(editorTheme(readEditorTheme(hostRef.current))),
          EditorView.updateListener.of((update) => {
            if (
              update.docChanged &&
              !update.transactions.some((tr) => tr.annotation(remoteUpdate))
            ) {
              const { lineSeparator } = values.model.getSnapshot();
              values.model.edit(update.state.doc.sliceString(0, undefined, lineSeparator));
            }
            if (update.selectionSet || update.docChanged) {
              const head = update.state.selection.main.head;
              const line = update.state.doc.lineAt(head);
              onCursorChangeRef.current({ line: line.number, column: head - line.from + 1 });
            }
          }),
        ],
      }),
    });
    viewRef.current = view;
    onCursorChangeRef.current({ line: 1, column: 1 });
    return () => {
      view.destroy();
      viewRef.current = null;
    };
    // Mount-only: the view is created once, and every later fact arrives
    // through the effects below — recreating it per snapshot would drop
    // the cursor and the undo history on every keystroke's echo.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Disk content (a reload, an outside edit adopted while clean) replaces
  // the document without touching history: typing stays undoable across
  // a refresh, and a refresh never reads as the user's own edit.
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    const document = view.state.toText(snapshot.content);
    if (view.state.doc.eq(document)) return;
    const head = Math.min(view.state.selection.main.head, document.length);
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: document },
      selection: { anchor: head },
      annotations: [remoteUpdate.of(true), Transaction.addToHistory.of(false)],
    });
  }, [snapshot.content]);

  // A line target lands the cursor and scrolls it to the middle; without
  // one the view keeps whatever the user last looked at.
  useEffect(() => {
    const view = viewRef.current;
    if (!view || !lineStart) return;
    const first = Math.min(lineStart, view.state.doc.lines);
    const last = Math.min(lineEnd ?? first, view.state.doc.lines);
    const from = view.state.doc.line(first).from;
    const to = view.state.doc.line(Math.max(first, last)).to;
    view.dispatch({
      selection: { anchor: from, head: last > first ? to : from },
      effects: EditorView.scrollIntoView(from, { y: "center" }),
    });
  }, [lineStart, lineEnd, navigationRevision]);

  return (
    <div className="file-editor-frame">
      <div
        ref={hostRef}
        data-testid="file-source-editor"
        aria-label={`Source editor for ${filename}`}
        className="file-editor-host"
      />
    </div>
  );
}

const remoteUpdate = Annotation.define<boolean>();
