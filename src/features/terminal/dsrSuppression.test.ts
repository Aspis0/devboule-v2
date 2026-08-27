import { describe, expect, it } from "vitest";
import { Terminal } from "@xterm/xterm";
import { suppressAutomaticDsrReplies } from "./terminalDsr";

async function write(terminal: Terminal, data: string): Promise<void> {
  await new Promise<void>((resolve) => terminal.write(data, resolve));
}

describe("xterm DSR handling", () => {
  it("suppresses automatic CPR replies while preserving user onData", async () => {
    const terminal = new Terminal({ cols: 10, rows: 2 });
    const data: string[] = [];
    terminal.onData((value) => data.push(value));
    const disposables = suppressAutomaticDsrReplies(terminal);

    await write(terminal, "\x1b[6n\x1b[?6n");
    expect(data).toEqual([]);

    terminal.input("typed", true);
    expect(data).toEqual(["typed"]);

    for (const disposable of disposables) disposable.dispose();
    await write(terminal, "\x1b[6n");
    expect(data).toEqual(["typed", "\x1b[1;1R"]);

    terminal.dispose();
  });
});
