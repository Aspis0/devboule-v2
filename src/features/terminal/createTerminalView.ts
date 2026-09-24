// This module is loaded only through the terminal surface's dynamic import so
// the xterm runtime and stylesheet stay out of the initial application chunk.
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { THEME_CHANGE_EVENT } from "../../lib/theme";
import type { SessionSnapshot } from "../../types/ipc";
import { terminalKeyPolicy } from "./terminalKeyPolicy";
import { fitRowsCols } from "./terminalFit";
import { suppressAutomaticDsrReplies } from "./terminalDsr";

const SCROLLBACK = 5000;
const FONT_SIZE = 12;

interface PendingWriteContinuation {
  callback: () => void;
  completed: boolean;
}

function paletteColor(host: HTMLElement, variable: string): string {
  const hostColor = getComputedStyle(host).getPropertyValue(variable).trim();
  if (hostColor) return hostColor;
  return (
    getComputedStyle(document.documentElement).getPropertyValue(variable).trim() || "transparent"
  );
}

/**
 * The terminal palette, resolved through the supplied reader so it is pure and
 * testable. The ground is `--terminal-ground` — the one surface the host and
 * frame paint — for the background, the block cursor's under-colour and black;
 * anything else lets the viewport's own fill diverge from the frame and show
 * as a line at the box's edge (fix pass 2's measured 1px black line). Accent
 * and tones flip with `[data-theme="dark"]`.
 */
export function terminalTheme(color: (variable: string) => string) {
  return {
    background: color("--terminal-ground"),
    foreground: color("--code-text"),
    cursor: color("--accent"),
    cursorAccent: color("--terminal-ground"),
    selectionBackground: color("--fill-selected"),
    black: color("--terminal-ground"),
    red: color("--danger"),
    green: color("--tone-live"),
    yellow: color("--tone-attention"),
    blue: color("--tone-unattended"),
    magenta: color("--accent"),
    cyan: color("--tone-live"),
    white: color("--code-text"),
    brightBlack: color("--tone-idle"),
    brightRed: color("--danger"),
    brightGreen: color("--tone-live"),
    brightYellow: color("--tone-attention"),
    brightBlue: color("--tone-unattended"),
    brightMagenta: color("--accent"),
    brightCyan: color("--tone-live"),
    brightWhite: color("--lb-text"),
  };
}

export interface CreateTerminalViewOptions {
  /** User input, paste, and terminal replies other than suppressed DSR CPRs. */
  onData: (data: string) => void;
  /** Route plain Ctrl+C through the controller's arm/confirm guard. */
  onCtrlC: () => void;
  /**
   * The session's resize request, called when the document's fonts settle
   * after the opening fit. Only `TerminalSession.doResize` sends
   * `session_resize`, so the font refit must ride the session's
   * `requestResize` path — a direct fit here would leave xterm and the shell
   * on different grids.
   */
  onFontFit?: () => void;
}

export interface TerminalViewHandle {
  write: (data: string, callback?: () => void) => void;
  applySnapshot: (snapshot: SessionSnapshot, callback: () => void) => void;
  fit: () => boolean;
  dispose: () => void;
  cols: () => number;
  rows: () => number;
}

/**
 * DECSCUSR: 1, 3 and 5 are the blinking block, underline and bar; the even code
 * above each one is its steady twin. The snapshot carries `blinking`, so the
 * sequence must select the right half of the pair rather than always blink.
 */
function cursorShapeCode(cursor: SessionSnapshot["cursor"]): number {
  const steady = cursor.blinking ? 0 : 1;
  switch (cursor.shape) {
    case "block":
      return 1 + steady;
    case "underline":
      return 3 + steady;
    case "bar":
      return 5 + steady;
  }
}

function snapshotStateSequence(snapshot: SessionSnapshot): string {
  const cursor = snapshot.cursor;
  const alternateScreen = snapshot.alternateScreen ? "h" : "l";
  const visible = cursor.visible ? "h" : "l";
  const bracketedPaste = snapshot.bracketedPaste ? "h" : "l";
  const lineWrap = snapshot.lineWrap ? "h" : "l";
  const title = snapshot.title === undefined ? "" : `\x1b]2;${snapshot.title}\x1b\\`;
  return [
    `\x1b[?1049${alternateScreen}`,
    `\x1b[${cursor.row + 1};${cursor.col + 1}H`,
    `\x1b[?25${visible}`,
    `\x1b[${cursorShapeCode(cursor)} q`,
    `\x1b[?2004${bracketedPaste}`,
    `\x1b[?7${lineWrap}`,
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
  const themeFromHost = () => terminalTheme((variable) => paletteColor(host, variable));
  const terminal = new Terminal({
    // Keep stdin enabled for user onData; the parser handler below suppresses
    // only automatic CPR replies so the daemon remains the single responder.
    disableStdin: false,
    scrollback: SCROLLBACK,
    fontSize: FONT_SIZE,
    fontFamily: 'JetBrains Mono, "Fira Code", Menlo, Consolas, monospace',
    convertEol: false,
    theme: themeFromHost(),
  });

  const dsrDisposables = suppressAutomaticDsrReplies(terminal);
  terminal.attachCustomKeyEventHandler((event) => terminalKeyPolicy(event, options.onCtrlC));

  // xterm snapshots its palette at theme-application time, so a theme change
  // after the view opened must be re-read. The theme module announces every
  // application; re-reading the CSS custom properties here (not caching them)
  // is what makes a live terminal follow the switch. Guarded because this
  // module also runs against a stubbed, event-less document in unit tests.
  const followsTheme =
    typeof document !== "undefined" && typeof document.addEventListener === "function";
  const reapplyTheme = () => {
    if (disposed) return;
    terminal.options.theme = themeFromHost();
  };
  if (followsTheme) {
    document.addEventListener(THEME_CHANGE_EVENT, reapplyTheme);
  }

  const fitAddon = new FitAddon();
  terminal.loadAddon(fitAddon);
  const dataDisposable = terminal.onData(options.onData);

  // FitAddon reads the host's border box and subtracts only the terminal
  // element's own padding, so any padding it counts as content overfills the
  // box and clips the last row and column (fix pass 1's measured defect). The
  // host is padding-free by contract now; this re-check measures the rendered
  // screen against the host's content box and re-fits through the pure
  // arithmetic if it ever overflows again, so a future box or addon change
  // degrades to a clamped fit, never a clipped prompt. Cell metrics do not
  // depend on rows/cols, so a mid-render measurement is still the right cell.
  // The addon's proposal — always present here, because runFit skips any fit
  // without one — is the upper bound: it reserves the scrollbar's width,
  // which the raw content-box arithmetic does not see, so the clamp can
  // shrink, never widen.
  const clampToFitBox = (proposal: { cols: number; rows: number }): void => {
    if (typeof host.querySelector !== "function") return; // stubbed hosts in tests
    if (terminal.rows === 0 || terminal.cols === 0) return;
    const screen: unknown = host.querySelector(".xterm-screen");
    if (
      screen === null ||
      typeof (screen as { getBoundingClientRect?: unknown }).getBoundingClientRect !== "function"
    ) {
      return;
    }
    const screenBox = (screen as { getBoundingClientRect: () => DOMRect }).getBoundingClientRect();
    if (typeof getComputedStyle !== "function") return;
    const hostStyle = getComputedStyle(host);
    const padX =
      Number.parseFloat(hostStyle.getPropertyValue("padding-left")) +
      Number.parseFloat(hostStyle.getPropertyValue("padding-right"));
    const padY =
      Number.parseFloat(hostStyle.getPropertyValue("padding-top")) +
      Number.parseFloat(hostStyle.getPropertyValue("padding-bottom"));
    const content = {
      width: host.clientWidth - padX,
      height: host.clientHeight - padY,
    };
    if (!Number.isFinite(content.width) || !Number.isFinite(content.height)) return;
    const cell = {
      width: screenBox.width / terminal.cols,
      height: screenBox.height / terminal.rows,
    };
    if (
      !Number.isFinite(cell.width) ||
      !Number.isFinite(cell.height) ||
      cell.width <= 0 ||
      cell.height <= 0
    ) {
      return;
    }
    const fitted = fitRowsCols(content, cell);
    const cols = Math.min(fitted.cols, proposal.cols);
    const rows = Math.min(fitted.rows, proposal.rows);
    if (cols !== terminal.cols || rows !== terminal.rows) {
      terminal.resize(cols, rows);
    }
  };

  const runFit = (): boolean => {
    // A collapsed or hidden host has no grid to keep: skip the fit and send
    // nothing, so the PTY keeps its last good geometry instead of a minimum
    // 2×1 grid.
    if (host.clientWidth <= 0 || host.clientHeight <= 0) return false;
    // The addon returns undefined when it cannot propose (no laid-out element,
    // or zero cell metrics). With no proposal there is no scrollbar-aware
    // bound: no fit.
    const proposal = fitAddon.proposeDimensions();
    if (proposal == null || proposal.cols <= 0 || proposal.rows <= 0) return false;
    try {
      fitAddon.fit();
      clampToFitBox(proposal);
      return true;
    } catch {
      return false;
    }
  };

  let disposed = false;

  terminal.open(host);
  // The construction-time palette read can race the stylesheet: until the
  // tokens resolve, every variable reads empty and xterm paints its default
  // black viewport — the line that showed at the box's edge (fix pass 2). By
  // open the document is styled, so the theme is applied again from the same
  // tokens.
  reapplyTheme();
  // A hidden host can have zero dimensions; ResizeObserver will retry later.
  runFit();
  // The bundled JetBrains Mono can finish loading after the first fit: the
  // cell size changes while the host box does not, so the box observer never
  // fires and the grid keeps the fallback font's shape. The refit rides the
  // session's requestResize path (via `onFontFit`), because only
  // TerminalSession.doResize sends `session_resize` — a direct fit here would
  // leave the shell wrapping at the old grid.
  if (typeof document !== "undefined" && "fonts" in document) {
    void document.fonts.ready.then(() => {
      if (!disposed) options.onFontFit?.();
    });
  }

  const pendingWriteContinuations = new Set<PendingWriteContinuation>();

  const completeWrite = (continuation: PendingWriteContinuation): void => {
    if (continuation.completed) return;
    continuation.completed = true;
    pendingWriteContinuations.delete(continuation);
    continuation.callback();
  };

  const writeWithCallback = (data: string, callback?: () => void): void => {
    if (callback === undefined) {
      if (!disposed) terminal.write(data);
      return;
    }

    const continuation: PendingWriteContinuation = { callback, completed: false };
    pendingWriteContinuations.add(continuation);
    if (disposed) {
      completeWrite(continuation);
      return;
    }
    terminal.write(data, () => completeWrite(continuation));
  };

  const completePendingWrites = (): void => {
    // xterm does not invoke pending write callbacks after disposal; complete
    // them here so session continuations cannot remain suspended forever.
    for (const continuation of [...pendingWriteContinuations]) completeWrite(continuation);
  };

  return {
    write: (data, callback) => {
      writeWithCallback(data, callback);
    },
    applySnapshot: (snapshot, callback) => {
      if (disposed) {
        callback();
        return;
      }
      terminal.reset();
      terminal.resize(snapshot.cols, snapshot.rows);
      writeWithCallback(snapshot.data, () => {
        if (disposed) {
          callback();
          return;
        }
        // The state sequence is written only after the snapshot data has been
        // parsed. The session callback therefore cannot release input early.
        writeWithCallback(snapshotStateSequence(snapshot), () => {
          terminal.options.cursorBlink = snapshot.cursor.blinking;
          callback();
        });
      });
    },
    fit: () => {
      if (disposed) return false;
      return runFit();
    },
    dispose: () => {
      if (disposed) return;
      disposed = true;
      if (followsTheme) {
        document.removeEventListener(THEME_CHANGE_EVENT, reapplyTheme);
      }
      completePendingWrites();
      for (const disposable of dsrDisposables) disposable.dispose();
      dataDisposable.dispose();
      terminal.dispose();
    },
    cols: () => terminal.cols,
    rows: () => terminal.rows,
  };
}
