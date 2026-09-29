// @vitest-environment happy-dom
import { describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";
import {
  isDeviceAttributesQuery,
  isDeviceStatusQuery,
  isWindowReport,
  suppressAutomaticQueryReplies,
} from "./terminalQuerySuppression";

async function write(terminal: Terminal, data: string): Promise<void> {
  await new Promise<void>((resolve) => terminal.write(data, resolve));
}

/**
 * The product's own Terminal options — windowOptions stays at its default,
 * and the view opens its terminal, which is what the colour replies need.
 */
function probeTerminal(opened: boolean): Terminal {
  const terminal = new Terminal({ cols: 20, rows: 4 });
  if (opened) {
    const host = document.createElement("div");
    document.body.appendChild(host);
    terminal.open(host);
  }
  return terminal;
}

const AUTO_ANSWERED: Array<[name: string, query: string, opened: boolean]> = [
  ["DA1", "\x1b[c", false],
  ["DA1 zero parameter", "\x1b[0c", false],
  ["DA2", "\x1b[>c", false],
  ["DSR 5", "\x1b[5n", false],
  ["CPR", "\x1b[6n", false],
  ["DECXCPR", "\x1b[?6n", false],
  ["DECRQM ansi mode", "\x1b[25$p", false],
  ["DECRQM private mode", "\x1b[?2026$p", false],
  ["DECRQSS", "\x1bP$q\x1b\\", false],
  ["OSC 4 colour query", "\x1b]4;1;?\x1b\\", true],
  ["OSC 10 foreground query", "\x1b]10;?\x07", true],
  ["OSC 11 background query", "\x1b]11;?\x07", true],
  ["OSC 12 cursor query", "\x1b]12;?\x07", true],
];

/** The classes the daemon answers on the PTY, so xterm's reply must go. */
const DAEMON_ANSWERED: Array<[name: string, query: string]> = [
  ["DA1", "\x1b[c"],
  ["DA1 zero parameter", "\x1b[0c"],
  ["DA2", "\x1b[>c"],
  ["DSR 5", "\x1b[5n"],
  ["CPR", "\x1b[6n"],
  ["DECRQM ansi mode", "\x1b[25$p"],
  ["DECRQM private mode", "\x1b[?2026$p"],
];

/** The classes no daemon answers: xterm stays the one responder. */
const XTERM_ANSWERED: Array<[name: string, query: string]> = [
  ["DECXCPR", "\x1b[?6n"],
  ["DECRQSS", "\x1bP$q\x1b\\"],
  ["OSC 4 colour query", "\x1b]4;1;?\x1b\\"],
  ["OSC 10 foreground query", "\x1b]10;?\x07"],
  ["OSC 11 background query", "\x1b]11;?\x07"],
  ["OSC 12 cursor query", "\x1b]12;?\x07"],
];

describe("xterm auto-answers protocol queries on its own (measured)", () => {
  it.each(AUTO_ANSWERED)("emits a reply for %s through onData", async (_name, query, opened) => {
    const terminal = probeTerminal(opened);
    const data: string[] = [];
    terminal.onData((value) => data.push(value));

    await write(terminal, query);

    expect(data.length).toBeGreaterThan(0);
    terminal.dispose();
  });
});

describe("the classes the daemon answers: xterm's reply is consumed", () => {
  it.each(DAEMON_ANSWERED)("leaves onData empty for %s", async (_name, query) => {
    const terminal = probeTerminal(false);
    const data: string[] = [];
    terminal.onData((value) => data.push(value));
    const disposables = suppressAutomaticQueryReplies(terminal);

    await write(terminal, query);

    expect(data).toEqual([]);
    for (const disposable of disposables) disposable.dispose();
    terminal.dispose();
  });

  it("keeps consuming the CPR form after disposal of other handlers", async () => {
    const terminal = probeTerminal(false);
    const data: string[] = [];
    terminal.onData((value) => data.push(value));
    const disposables = suppressAutomaticQueryReplies(terminal);

    await write(terminal, "\x1b[6n");
    expect(data).toEqual([]);

    for (const disposable of disposables) disposable.dispose();
    await write(terminal, "\x1b[6n");
    expect(data).toEqual(["\x1b[1;1R"]);
    terminal.dispose();
  });

  it("still lets user input through while suppressing", async () => {
    const terminal = probeTerminal(false);
    const data: string[] = [];
    terminal.onData((value) => data.push(value));
    const disposables = suppressAutomaticQueryReplies(terminal);

    terminal.input("typed", true);
    expect(data).toEqual(["typed"]);

    for (const disposable of disposables) disposable.dispose();
    terminal.dispose();
  });
});

describe("the classes only xterm answers keep their reply", () => {
  it.each(XTERM_ANSWERED)(
    "still answers %s with the suppression registered",
    async (_name, query) => {
      const terminal = probeTerminal(query.startsWith("\x1b]"));
      const data: string[] = [];
      terminal.onData((value) => data.push(value));
      const disposables = suppressAutomaticQueryReplies(terminal);

      await write(terminal, query);

      expect(data.length).toBeGreaterThan(0);
      for (const disposable of disposables) disposable.dispose();
      terminal.dispose();
    },
  );
});

describe("colour sequences that set", () => {
  /** Registered before the suppression: sees only what the suppression declines. */
  function probeOscFour(terminal: Terminal): string[] {
    const seen: string[] = [];
    terminal.parser.registerOscHandler(4, (payload) => {
      seen.push(payload);
      return false;
    });
    return seen;
  }

  it("keeps a plain colour set's effect on xterm", async () => {
    const terminal = probeTerminal(true);
    const data: string[] = [];
    terminal.onData((value) => data.push(value));
    const seen = probeOscFour(terminal);
    const disposables = suppressAutomaticQueryReplies(terminal);

    await write(terminal, "\x1b]4;1;#ff0000\x07");

    expect(seen.length).toBe(1);
    expect(seen[0]).toContain("#ff0000");
    expect(data).toEqual([]);
    for (const disposable of disposables) disposable.dispose();
    terminal.dispose();
  });

  it("keeps the set half of a mixed set+query payload", async () => {
    const terminal = probeTerminal(true);
    const data: string[] = [];
    terminal.onData((value) => data.push(value));
    const seen = probeOscFour(terminal);
    const disposables = suppressAutomaticQueryReplies(terminal);

    await write(terminal, "\x1b]4;1;?;2;#00ff00\x07");

    expect(seen.length).toBe(1);
    expect(seen[0]).toContain("#00ff00");
    for (const disposable of disposables) disposable.dispose();
    terminal.dispose();
  });
});

describe("the query decision table", () => {
  it("takes DA1/DA2 only in the parameter form xterm answers", () => {
    expect(isDeviceAttributesQuery([0])).toBe(true);
    expect(isDeviceAttributesQuery([1])).toBe(false);
  });

  it("takes DSR only for the statuses xterm answers", () => {
    expect(isDeviceStatusQuery([5])).toBe(true);
    expect(isDeviceStatusQuery([6])).toBe(true);
    expect(isDeviceStatusQuery([15])).toBe(false);
  });

  it("takes only the window report the daemon answers", () => {
    expect(isWindowReport([18])).toBe(true);
    expect(isWindowReport([14])).toBe(false);
    expect(isWindowReport([16])).toBe(false);
    expect(isWindowReport([22])).toBe(false);
    expect(isWindowReport([23])).toBe(false);
    expect(isWindowReport([8])).toBe(false);
  });
});

describe("the sixth suppressed class: CSI 18 t", () => {
  /**
   * The product leaves windowOptions off, so xterm answers 18 t only after a
   * future opt-in — the day that happens, this suppression is the line
   * between one answer and two, and only this config can see it.
   */
  function sizeQueryTerminal(): Terminal {
    const terminal = new Terminal({
      cols: 20,
      rows: 4,
      windowOptions: { getWinSizeChars: true },
    });
    const host = document.createElement("div");
    document.body.appendChild(host);
    terminal.open(host);
    return terminal;
  }

  it("measures that xterm answers CSI 18 t with the option on", async () => {
    const terminal = sizeQueryTerminal();
    const data: string[] = [];
    terminal.onData((value) => data.push(value));

    await write(terminal, "\x1b[18t");

    expect(data).toEqual(["\x1b[8;4;20t"]);
    terminal.dispose();
  });

  it("leaves onData empty for CSI 18 t through the real registration", async () => {
    const terminal = sizeQueryTerminal();
    const data: string[] = [];
    terminal.onData((value) => data.push(value));
    const disposables = suppressAutomaticQueryReplies(terminal);

    await write(terminal, "\x1b[18t");

    expect(data).toEqual([]);
    for (const disposable of disposables) disposable.dispose();
    terminal.dispose();
  });
});
