// Ported from Paseo's `packages/app/src/file-pane/editor/extensions.web.ts`
// (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra): the CodeMirror 6
// base extensions (line numbers, history, Tab indent, Ctrl/Cmd+S) and the
// theme hook. Two deviations, both deliberate: no per-language grammars
// (Paseo's `@getpaseo/highlight` has no equivalent here, so the default
// highlight style is the only one — plain text still indents, matches
// brackets and numbers lines), and no vim mode (out of scope for this
// slice). Colours come from this app's own tokens, read off the host
// element, so the editor follows the theme without a Paseo palette.

import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import {
  bracketMatching,
  defaultHighlightStyle,
  indentOnInput,
  syntaxHighlighting,
} from "@codemirror/language";
import {
  EditorView,
  drawSelection,
  highlightActiveLine,
  keymap,
  lineNumbers,
} from "@codemirror/view";

export interface EditorVisualTheme {
  background: string;
  foreground: string;
  cursor: string;
  foregroundMuted: string;
  border: string;
  selection: string;
  monoFont: string;
  codeFontSize: number;
  dark: boolean;
}

export function editorBaseExtensions(onSave: () => void) {
  return [
    lineNumbers(),
    history(),
    drawSelection(),
    indentOnInput(),
    bracketMatching(),
    highlightActiveLine(),
    syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
    keymap.of([
      { key: "Mod-s", preventDefault: true, run: () => (onSave(), true) },
      indentWithTab,
      ...defaultKeymap,
      ...historyKeymap,
    ]),
  ];
}

export function editorTheme(theme: EditorVisualTheme) {
  return EditorView.theme(
    {
      "&": {
        height: "100%",
        backgroundColor: theme.background,
        color: theme.foreground,
        fontFamily: theme.monoFont,
        fontSize: `${theme.codeFontSize}px`,
      },
      ".cm-scroller": { overflow: "auto", fontFamily: theme.monoFont, lineHeight: "1.45" },
      ".cm-content": { caretColor: theme.foreground, padding: "12px 0" },
      ".cm-cursor, .cm-dropCursor": { borderLeftColor: theme.cursor },
      ".cm-gutters": {
        backgroundColor: theme.background,
        color: theme.foregroundMuted,
        borderRight: `1px solid ${theme.border}`,
      },
      ".cm-activeLine": { backgroundColor: "transparent" },
      ".cm-activeLineGutter": { backgroundColor: "transparent", color: theme.foreground },
      "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
        backgroundColor: theme.selection,
      },
      ".cm-selectionBackground, ::selection": { backgroundColor: theme.selection },
      "&.cm-focused": { outline: "none" },
    },
    { dark: theme.dark },
  );
}

/** Read this app's tokens off the host: the editor follows the theme and
 * names no colour of its own. Every read falls back to a plain dark pair,
 * so a missing token is never a crash — the terminal reads its host the
 * same way (`createTerminalView`). */
export function readEditorTheme(host: HTMLElement): EditorVisualTheme {
  const root = getComputedStyle(document.documentElement);
  const read = (variable: string, fallback: string): string => {
    const value =
      getComputedStyle(host).getPropertyValue(variable).trim() ||
      root.getPropertyValue(variable).trim();
    return value === "" ? fallback : value;
  };
  return {
    background: read("--panel-card", "#1e1e1e"),
    foreground: read("--ink", "#e8e7e2"),
    cursor: read("--accent", "#e8e7e2"),
    foregroundMuted: read("--muted", "#8a887f"),
    border: read("--line", "#353532"),
    selection: read("--code-selection", "rgba(232, 231, 226, 0.28)"),
    monoFont: read("--font-mono", "ui-monospace, monospace"),
    codeFontSize: Number.parseFloat(read("--code-size", "13")) || 13,
    dark: document.documentElement.getAttribute("data-theme") !== "light",
  };
}
