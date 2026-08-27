import { Terminal, type IDisposable } from "@xterm/xterm";

function isCursorPositionReport(params: readonly (number | number[])[]): boolean {
  return params.length === 1 && params[0] === 6;
}

/**
 * xterm normally answers CPR requests through onData. The daemon owns that
 * response now, so consume only the two CPR forms while keeping user onData.
 */
export function suppressAutomaticDsrReplies(terminal: Terminal): IDisposable[] {
  const suppressCpr = (params: (number | number[])[]) => isCursorPositionReport(params);
  return [
    terminal.parser.registerCsiHandler({ final: "n" }, suppressCpr),
    terminal.parser.registerCsiHandler({ prefix: "?", final: "n" }, suppressCpr),
  ];
}
