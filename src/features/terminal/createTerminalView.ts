// This module is loaded only through the terminal surface's dynamic import so
// the xterm runtime and stylesheet stay out of the initial application chunk.
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import type { SessionSnapshot } from "../../types/ipc";
import { terminalKeyPolicy } from "./terminalKeyPolicy";
import { suppressAutomaticDsrReplies } from "./terminalDsr";

const SCROLLBACK = 5000;
const FONT_SIZE = 12;

function paletteColor(host: HTMLElement, variable: string): string {
  const hostColor = getComputedStyle(host).getPropertyValue(variable).trim();
  if (hostColor) return hostColor;
  return (
    getComputedStyle(document.documentElement).getPropertyValue(variable).trim() || "transparent"
  );
}

function terminalTheme(host: HTMLElement) {
  const color = (variable: string) => paletteColor(host, variable);

  return {
    background: color("--ink"),
    foreground: color("--surface"),
    cursor: color("--terracotta"),
    cursorAccent: color("--ink"),
    selectionBackground: color("--selection"),
    black: color("--ink"),
    red: color("--danger"),
    green: color("--green"),
    yellow: color("--ochre"),
    blue: color("--purple"),
    magenta: color("--terracotta-deep"),
    cyan: color("--green"),
    white: color("--surface"),
    brightBlack: color("--terminal-silence"),
    brightRed: color("--danger"),
    brightGreen: color("--green"),
    brightYellow: color("--ochre"),
    brightBlue: color("--purple"),
    brightMagenta: color("--terracotta"),
    brightCyan: color("--green"),
    brightWhite: color("--white"),
  };
}

export interface CreateTerminalViewOptions {
  /** User input, paste, and terminal replies other than suppressed DSR CPRs. */
  onData: (data: string) => void;
  /** Route plain Ctrl+C through the controller's arm/confirm guard. */
  onCtrlC: () => void;
}

export interface TerminalViewHandle {
  write: (data: string, callback?: () => void) => void;
  applySnapshot: (snapshot: SessionSnapshot, callback: () => void) => void;
  fit: () => boolean;
  dispose: () => void;
  cols: () => number;
  rows: () => number;
}

function cursorShapeCode(shape: SessionSnapshot["cursor"]["shape"]): number {
  switch (shape) {
    case "block":
      return 1;
    case "underline":
      return 3;
    case "bar":
      return 5;
  }
}

function snapshotStateSequence(snapshot: SessionSnapshot): string {
  const cursor = snapshot.cursor;
  const alternateScreen = snapshot.alternateScreen ? "h" : "l";
  const visible = cursor.visible ? "h" : "l";
  const bracketedPaste = snapshot.bracketedPaste ? "h" : "l";
  const title = snapshot.title === undefined ? "" : `\x1b]2;${snapshot.title}\x1b\\`;
  return [
    `\x1b[?1049${alternateScreen}`,
    `\x1b[${cursor.row + 1};${cursor.col + 1}H`,
    `\x1b[?25${visible}`,
    `\x1b[${cursorShapeCode(cursor.shape)} q`,
    `\x1b[?2004${bracketedPaste}`,
    title,
  ].join("");
}

/**
 * Mount an interactive xterm instance into the supplied host.
 *
 * ConPTY emits a DSR cursor-position query (ESC[6n) at startup and stalls
 * until the daemon answers it. stdin remains enabled for real user input; the
 * parser hook above prevents xterm from becoming a second CPR responder. The
 * key policy controls user Ctrl+C without disabling stdin.
 */
export function createTerminalView(
  host: HTMLElement,
  options: CreateTerminalViewOptions,
): TerminalViewHandle {
  const terminal = new Terminal({
    // Keep stdin enabled for user onData; the parser handler below suppresses
    // only automatic CPR replies so the daemon remains the single responder.
    disableStdin: false,
    scrollback: SCROLLBACK,
    fontSize: FONT_SIZE,
    fontFamily: 'JetBrains Mono, "Fira Code", Menlo, Consolas, monospace',
    cursorBlink: false,
    convertEol: false,
    theme: terminalTheme(host),
  });

  const dsrDisposables = suppressAutomaticDsrReplies(terminal);
  terminal.attachCustomKeyEventHandler((event) => terminalKeyPolicy(event, options.onCtrlC));

  const fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  const dataDisposable = terminal.onData(options.onData);

  terminal.open(host);
  try {
    fitAddon.fit();
  } catch {
    // A hidden host can have zero dimensions; ResizeObserver will retry later.
  }

  let disposed = false;

  return {
    write: (data, callback) => {
      if (!disposed) terminal.write(data, callback);
    },
    applySnapshot: (snapshot, callback) => {
      if (disposed) return;
      terminal.reset();
      terminal.resize(snapshot.cols, snapshot.rows);
      terminal.write(snapshot.data, () => {
        if (disposed) return;
        // The state sequence is written only after the snapshot data has been
        // parsed. The session callback therefore cannot release input early.
        terminal.write(snapshotStateSequence(snapshot), callback);
      });
    },
    fit: () => {
      if (disposed) return false;
      try {
        fitAddon.fit();
        return true;
      } catch {
        return false;
      }
    },
    dispose: () => {
      if (disposed) return;
      disposed = true;
      for (const disposable of dsrDisposables) disposable.dispose();
      dataDisposable.dispose();
      terminal.dispose();
    },
    cols: () => terminal.cols,
    rows: () => terminal.rows,
  };
}
