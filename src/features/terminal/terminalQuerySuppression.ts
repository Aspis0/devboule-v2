import { Terminal, type IDisposable } from "@xterm/xterm";

function firstParam(params: (number | number[])[]): number {
  const value = params[0];
  return typeof value === "number" ? value : Number.NaN;
}

/** xterm answers DA1/DA2 only when the first parameter is absent or zero. */
export function isDeviceAttributesQuery(params: (number | number[])[]): boolean {
  return !(firstParam(params) > 0);
}

/** DSR 5 (ready) and 6 (cursor position) — the forms xterm replies to. */
export function isDeviceStatusQuery(params: (number | number[])[]): boolean {
  const param = firstParam(params);
  return param === 5 || param === 6;
}

/**
 * CSI 18 t (text area size in chars) is the one window report the daemon
 * answers. The pixel reports 14/16 t have no daemon answer and stay with
 * xterm, as do the title stack ops 22/23 t — they share the final byte and
 * change state, so they must keep reaching xterm's own handler.
 */
export function isWindowReport(params: (number | number[])[]): boolean {
  return firstParam(params) === 18;
}

/**
 * One responder per query class: every query gets exactly one answer.
 *
 * xterm 6.0.0 answers protocol queries itself and emits the reply through
 * onData, which this view forwards to the PTY — a second answer for any class
 * the daemon also answers, arriving as typed input once the querying app has
 * exited. So xterm's reply is suppressed ONLY for the classes the daemon
 * really answers; every other class keeps xterm as its one responder.
 *
 * Measured: @xterm/xterm 6.0.0 lib/xterm.mjs (client side) and
 * alacritty_terminal 0.26.0 + vte 0.15.0 (daemon side).
 *
 *   query                       daemon    xterm client
 *   CSI c        (DA1)          answers   suppressed
 *   CSI > c      (DA2)          answers   suppressed
 *   CSI 5 n      (DSR 5)        answers   suppressed
 *   CSI 6 n      (CPR)          answers   suppressed
 *   CSI Ps $ p   (DECRQM)       answers   suppressed
 *   CSI ? Ps $ p (DECRQM)       answers   suppressed
 *   CSI 18 t     (text size)    answers   suppressed
 *   CSI ? 6 n    (DECXCPR)      —         answers
 *   DCS $ q      (DECRQSS)      —         answers
 *   OSC 4/10/11/12 ;?  (colour) —         answers
 *   CSI 14 t / 16 t (pixels)    —         only with windowOptions, which this
 *                                         product leaves off — nobody answers
 *                                         today, pre-existing, follow-up
 *   CSI = c      (DA3)          —         — (neither answers; nothing to suppress)
 */
export function suppressAutomaticQueryReplies(terminal: Terminal): IDisposable[] {
  const consume = (): boolean => true;
  return [
    terminal.parser.registerCsiHandler({ final: "c" }, isDeviceAttributesQuery),
    terminal.parser.registerCsiHandler({ prefix: ">", final: "c" }, isDeviceAttributesQuery),
    terminal.parser.registerCsiHandler({ final: "n" }, isDeviceStatusQuery),
    terminal.parser.registerCsiHandler({ final: "t" }, isWindowReport),
    // DECRQM answers every mode query, recognised or not.
    terminal.parser.registerCsiHandler({ intermediates: "$", final: "p" }, consume),
    terminal.parser.registerCsiHandler({ prefix: "?", intermediates: "$", final: "p" }, consume),
  ];
}
