// This module is loaded only through the terminal surface's dynamic import so
// the xterm runtime and stylesheet stay out of the initial application chunk.
import { FitAddon } from '@xterm/addon-fit';
import { Terminal } from '@xterm/xterm';
import '@xterm/xterm/css/xterm.css';
import { terminalKeyPolicy } from './terminalKeyPolicy';

const SCROLLBACK = 5000;
const FONT_SIZE = 12;

function paletteColor(host: HTMLElement, variable: string): string {
  const hostColor = getComputedStyle(host).getPropertyValue(variable).trim();
  if (hostColor) return hostColor;
  return getComputedStyle(document.documentElement).getPropertyValue(variable).trim() || 'transparent';
}

function terminalTheme(host: HTMLElement) {
  const color = (variable: string) => paletteColor(host, variable);

  return {
    background: color('--ink'),
    foreground: color('--surface'),
    cursor: color('--terracotta'),
    cursorAccent: color('--ink'),
    selectionBackground: color('--selection'),
    black: color('--ink'),
    red: color('--danger'),
    green: color('--green'),
    yellow: color('--ochre'),
    blue: color('--purple'),
    magenta: color('--terracotta-deep'),
    cyan: color('--green'),
    white: color('--surface'),
    brightBlack: color('--silence'),
    brightRed: color('--danger'),
    brightGreen: color('--green'),
    brightYellow: color('--ochre'),
    brightBlue: color('--purple'),
    brightMagenta: color('--terracotta'),
    brightCyan: color('--green'),
    brightWhite: color('--white'),
  };
}

export interface CreateTerminalViewOptions {
  /** User input, paste, and xterm's automatic terminal replies. */
  onData: (data: string) => void;
  /** Route plain Ctrl+C through the controller's arm/confirm guard. */
  onCtrlC: () => void;
}

export interface TerminalViewHandle {
  write: (data: string) => void;
  fit: () => boolean;
  dispose: () => void;
  cols: () => number;
  rows: () => number;
}

/**
 * Mount an interactive xterm instance into the supplied host.
 *
 * ConPTY emits a DSR cursor-position query (ESC[6n) at startup and stalls
 * until the terminal answers it. stdin MUST remain enabled: xterm's parser
 * answers the query through onData. The key policy controls user Ctrl+C; it
 * must not disable stdin, or a real shell can hang before its first prompt.
 */
export function createTerminalView(
  host: HTMLElement,
  options: CreateTerminalViewOptions,
): TerminalViewHandle {
  const terminal = new Terminal({
    // Keep stdin enabled so xterm can automatically answer ConPTY's DSR query.
    disableStdin: false,
    scrollback: SCROLLBACK,
    fontSize: FONT_SIZE,
    fontFamily: 'JetBrains Mono, "Fira Code", Menlo, Consolas, monospace',
    cursorBlink: false,
    convertEol: false,
    theme: terminalTheme(host),
  });

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
    write: (data) => {
      if (!disposed) terminal.write(data);
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
      dataDisposable.dispose();
      terminal.dispose();
    },
    cols: () => terminal.cols,
    rows: () => terminal.rows,
  };
}
