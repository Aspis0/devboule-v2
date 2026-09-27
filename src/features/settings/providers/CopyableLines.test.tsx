// @vitest-environment happy-dom

// One Copy button per exact line, with plain feedback and no timers.
// Used wherever the page hands a person a line to paste: the unknown-shell
// consent and the never-typed row notes.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { CopyableLines } from "./CopyableLines";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("CopyableLines", () => {
  let container: HTMLDivElement;
  let root: Root;
  const writes: string[] = [];

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    writes.length = 0;
    vi.stubGlobal("navigator", {
      ...navigator,
      clipboard: {
        writeText: vi.fn(async (text: string) => {
          writes.push(text);
        }),
      },
    });
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
    vi.clearAllMocks();
  });

  async function renderLines(
    lines: Array<{ label?: string | null; text: string }> = [
      { label: "Windows PowerShell", text: "install-a" },
      { label: "POSIX shells", text: "install-b" },
    ],
  ) {
    await act(async () => root.render(<CopyableLines lines={lines} />));
    await act(async () => undefined);
  }

  function copyButtons(): HTMLButtonElement[] {
    return Array.from(container.querySelectorAll<HTMLButtonElement>(".provider-copy-button"));
  }

  it("shows each line with its label and copies exactly that line", async () => {
    await renderLines();
    expect(container.textContent).toContain("Windows PowerShell");
    expect(container.textContent).toContain("install-a");
    expect(container.textContent).toContain("install-b");

    const buttons = copyButtons();
    expect(buttons).toHaveLength(2);
    await act(async () => buttons[1]?.click());
    expect(writes).toEqual(["install-b"]);
    expect(buttons[1]?.textContent).toBe("Copied");
    expect(buttons[0]?.textContent).toBe("Copy");
  });

  it("says plainly when the clipboard refuses", async () => {
    vi.stubGlobal("navigator", {
      ...navigator,
      clipboard: {
        writeText: vi.fn(async () => {
          throw new Error("denied");
        }),
      },
    });
    await renderLines([{ text: "install-a" }]);
    const buttons = copyButtons();
    await act(async () => buttons[0]?.click());
    expect(buttons[0]?.textContent).toBe("Copy failed");
  });

  it("says plainly when there is no clipboard at all", async () => {
    vi.stubGlobal("navigator", { ...navigator, clipboard: undefined });
    await renderLines([{ text: "install-a" }]);
    const buttons = copyButtons();
    await act(async () => buttons[0]?.click());
    expect(buttons[0]?.textContent).toBe("Copy failed");
  });
});
