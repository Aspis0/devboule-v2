import { describe, expect, it } from "vitest";
import { SerializeAddon } from "@xterm/addon-serialize";
import { Terminal } from "@xterm/xterm";

interface OracleCell {
  chars: string;
  width: number;
  fg: number;
  fgPalette: boolean;
  fgRgb: boolean;
}

interface OracleResult {
  serialized: string;
  activeType: "normal" | "alternate";
  bracketedPaste: boolean;
  cursorX: number;
  cursorY: number;
  lines: string[];
  cells: OracleCell[][];
}

async function write(terminal: Terminal, data: string): Promise<void> {
  await new Promise<void>((resolve) => terminal.write(data, resolve));
}

async function renderWithXterm(data: string, cols: number, rows: number): Promise<OracleResult> {
  const terminal = new Terminal({ cols, rows, scrollback: 20 });
  const serialize = new SerializeAddon();
  terminal.loadAddon(serialize);

  try {
    // The callback is required: xterm's public contract does not make the
    // buffer observable at the call site before the asynchronous parse ends.
    await write(terminal, data);
    const lines: string[] = [];
    const cells: OracleCell[][] = [];
    for (let row = 0; row < rows; row += 1) {
      const line = terminal.buffer.active.getLine(row);
      lines.push(line?.translateToString(false) ?? " ".repeat(cols));
      const rowCells: OracleCell[] = [];
      for (let col = 0; col < cols; col += 1) {
        const cell = line?.getCell(col);
        rowCells.push({
          chars: cell?.getChars() ?? "",
          width: cell?.getWidth() ?? 1,
          fg: cell?.getFgColor() ?? 0,
          fgPalette: cell?.isFgPalette() ?? false,
          fgRgb: cell?.isFgRGB() ?? false,
        });
      }
      cells.push(rowCells);
    }
    return {
      serialized: serialize.serialize(),
      activeType: terminal.buffer.active.type,
      bracketedPaste: terminal.modes.bracketedPasteMode,
      cursorX: terminal.buffer.active.cursorX,
      cursorY: terminal.buffer.active.cursorY,
      lines,
      cells,
    };
  } finally {
    serialize.dispose();
    terminal.dispose();
  }
}

describe("xterm ANSI oracle", () => {
  it("round-trips plain text and wrapping", async () => {
    const result = await renderWithXterm("hello world", 5, 3);

    expect(result.lines).toEqual(["hello", " worl", "d    "]);
    expect(result.cursorX).toBe(1);
    expect(result.cursorY).toBe(2);
    expect(result.serialized).toBe("hello world");
  });

  it("retains indexed and 24-bit SGR colours in the serialized oracle", async () => {
    const result = await renderWithXterm("\x1b[38;5;123mI\x1b[0m \x1b[38;2;1;2;3mR", 10, 2);

    expect(result.lines[0]).toBe("I R       ");
    expect(result.cells[0]?.[0]).toMatchObject({ fg: 123, fgPalette: true, fgRgb: false });
    expect(result.cells[0]?.[2]).toMatchObject({ fg: 0x010203, fgPalette: false, fgRgb: true });
    expect(result.serialized).toContain("\x1b[38;5;123m");
    expect(result.serialized).toContain("\x1b[38;2;1;2;3m");
  });

  it("keeps a CJK glyph at the final column on its wrapped row", async () => {
    const result = await renderWithXterm("abcd界", 5, 3);

    expect(result.lines).toEqual(["abcd ", "界   ", "     "]);
    expect(result.cells[1]?.[0]).toMatchObject({ chars: "界", width: 2 });
    expect(result.cells[1]?.[1]).toMatchObject({ chars: "", width: 0 });
    expect(result.cursorX).toBe(2);
    expect(result.cursorY).toBe(1);
    expect(result.serialized).toBe("abcd界");
  });

  it("keeps combining marks on the base cell", async () => {
    const result = await renderWithXterm("e\u0301x", 10, 2);

    expect(result.cells[0]?.[0]).toMatchObject({ chars: "e\u0301", width: 1 });
    expect(result.lines[0]).toBe("e\u0301x        ");
    expect(result.cursorX).toBe(2);
    expect(result.serialized).toBe("e\u0301x");
  });

  it("records the installed xterm emoji width behavior", async () => {
    const result = await renderWithXterm("🙂👩‍💻", 10, 2);

    // Known divergence to audit against alacritty: xterm 6.0.0's installed
    // default Unicode table treats standalone emoji as width 1, while many
    // terminal expectations treat emoji presentation as width 2.
    expect(result.cells[0]?.[0]).toMatchObject({ chars: "🙂", width: 1 });
    expect(result.cells[0]?.[1]).toMatchObject({ chars: "👩‍", width: 1 });
    expect(result.cells[0]?.[2]).toMatchObject({ chars: "💻", width: 1 });
    expect(result.cursorX).toBe(3);
    expect(result.serialized).toBe("🙂👩‍💻");
  });

  it("serializes the alternate screen and its bracketed paste mode", async () => {
    const result = await renderWithXterm("\x1b[?1049hALT\x1b[?2004h", 10, 3);

    expect(result.activeType).toBe("alternate");
    expect(result.bracketedPaste).toBe(true);
    expect(result.lines).toEqual(["ALT       ", "          ", "          "]);
    expect(result.serialized).toContain("\x1b[?1049h");
    expect(result.serialized).toContain("ALT");
    expect(result.serialized).toContain("\x1b[?2004h");
  });
});
