// @vitest-environment happy-dom

// The confirmation dialog itself: the tone rule (filled danger only for the
// destructive sole affirmative), the modal contract — focus in on Cancel, the
// trap, Escape, the scrim, focus back to the opener — and the copy it is
// given. The strip's four asks (terminal, running agent,
// counted selection, delete) are walked through the real Workspace in the
// harness tests; this file pins the dialog they all render. happy-dom does no
// layout, so no geometry is asserted here.

import { act, useState } from "react";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ConfirmDialog } from "./ConfirmDialog";
import { assembleCssProof } from "../features/workspace/cssProof";
import { useAppStore } from "../store/appStore";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const baseProps = {
  title: "Close terminal “shell two”?",
  message: "The process stops and every message stays in History.",
  confirmLabel: "Close",
  onConfirm: vi.fn(),
  onCancel: vi.fn(),
};

let container: HTMLDivElement;
let root: Root;

async function renderDialog(tone: "danger" | "accent" = "danger"): Promise<void> {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root.render(<ConfirmDialog {...baseProps} tone={tone} open />);
  });
}

function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>(".confirm-dialog");
  if (found === null) throw new Error("confirm dialog did not render");
  return found;
}

function confirmButton(): HTMLButtonElement {
  const found = document.querySelector<HTMLButtonElement>(".confirm-dialog-confirm");
  if (found === null) throw new Error("confirm button did not render");
  return found;
}

function cancelButton(): HTMLButtonElement {
  const found = document.querySelector<HTMLButtonElement>(".confirm-dialog-cancel");
  if (found === null) throw new Error("cancel button did not render");
  return found;
}

function backdrop(): HTMLElement {
  const found = document.querySelector<HTMLElement>(".confirm-dialog-backdrop");
  if (found === null) throw new Error("confirm backdrop did not render");
  return found;
}

function pressKey(target: HTMLElement, key: string, shiftKey = false): void {
  target.dispatchEvent(new KeyboardEvent("keydown", { key, shiftKey, bubbles: true }));
}

beforeEach(() => {
  useAppStore.setState({ modalOpenTokens: new Set() });
});

afterEach(async () => {
  await act(async () => {
    root.unmount();
  });
  container.remove();
  useAppStore.setState({ modalOpenTokens: new Set() });
  vi.clearAllMocks();
});

describe("the copy it is given", () => {
  it("renders the title, the body and both labels, and names them to assistive tech", async () => {
    await renderDialog();
    const card = dialog();
    expect(card.getAttribute("role")).toBe("alertdialog");
    expect(card.getAttribute("aria-modal")).toBe("true");
    expect(card.querySelector(".confirm-dialog-title")?.textContent).toBe(baseProps.title);
    expect(card.querySelector(".confirm-dialog-body")?.textContent).toBe(baseProps.message);
    expect(confirmButton().textContent).toBe("Close");
    expect(cancelButton().textContent).toBe("Cancel");
    const labelledby = card.getAttribute("aria-labelledby");
    const describedby = card.getAttribute("aria-describedby");
    expect(card.querySelector(`#${labelledby}`)?.textContent).toBe(baseProps.title);
    expect(card.querySelector(`#${describedby}`)?.textContent).toBe(baseProps.message);
  });
});

describe("the button tones", () => {
  it("the destructive ask's affirmative is the filled danger", async () => {
    await renderDialog("danger");
    expect(confirmButton().classList.contains("confirm-dialog-confirm-danger")).toBe(true);
    expect(confirmButton().classList.contains("confirm-dialog-confirm-accent")).toBe(false);
  });

  it("a non-destructive act's affirmative is the filled accent", async () => {
    await renderDialog("accent");
    expect(confirmButton().classList.contains("confirm-dialog-confirm-accent")).toBe(true);
    expect(confirmButton().classList.contains("confirm-dialog-confirm-danger")).toBe(false);
  });
});

describe("the modal contract", () => {
  it("opens with Cancel focused — Enter must never destroy", async () => {
    await renderDialog();
    expect(document.activeElement).toBe(cancelButton());
  });

  it("registers with the shell while it is up and releases on close", async () => {
    await renderDialog();
    expect(useAppStore.getState().modalOpenTokens.size).toBe(1);
    await act(async () => {
      root.render(<ConfirmDialog {...baseProps} tone="danger" open={false} />);
    });
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
  });

  it("Tab cycles inside the dialog and wraps; Shift+Tab runs back", async () => {
    await renderDialog();
    expect(document.activeElement).toBe(cancelButton());
    await act(async () => {
      pressKey(dialog(), "Tab");
    });
    expect(document.activeElement).toBe(confirmButton());
    await act(async () => {
      pressKey(dialog(), "Tab");
    });
    // Cycled: Tab from the last button lands back on the first.
    expect(document.activeElement).toBe(cancelButton());
    await act(async () => {
      pressKey(dialog(), "Tab", true);
    });
    expect(document.activeElement).toBe(confirmButton());
  });

  it("Escape cancels and does not confirm", async () => {
    await renderDialog();
    await act(async () => {
      pressKey(confirmButton(), "Escape");
    });
    expect(baseProps.onCancel).toHaveBeenCalledTimes(1);
    expect(baseProps.onConfirm).not.toHaveBeenCalled();
  });

  it("a press on the scrim cancels; a press on the card does not", async () => {
    await renderDialog();
    await act(async () => {
      backdrop().dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(baseProps.onCancel).toHaveBeenCalledTimes(1);
    expect(baseProps.onConfirm).not.toHaveBeenCalled();

    await act(async () => {
      root.unmount();
    });
    container.remove();
    vi.clearAllMocks();
    await renderDialog();
    await act(async () => {
      dialog().dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(baseProps.onCancel).not.toHaveBeenCalled();
    expect(baseProps.onConfirm).not.toHaveBeenCalled();
  });

  it("a press that starts in the card and ends on the scrim does not cancel", async () => {
    await renderDialog();
    await act(async () => {
      dialog().dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
      backdrop().dispatchEvent(new MouseEvent("mouseup", { bubbles: true }));
    });
    expect(baseProps.onCancel).not.toHaveBeenCalled();
    expect(baseProps.onConfirm).not.toHaveBeenCalled();
  });

  // The second half of a double-click must neither cancel nor move focus:
  // preventing the press's default keeps focus inside the card. happy-dom
  // performs no focus update on mousedown, so only the prevention half is
  // pinned here — the focus half was measured in a real browser.
  it("ignores the second press of a double-click on the scrim", async () => {
    await renderDialog();
    const second = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
      detail: 2,
    });
    await act(async () => {
      backdrop().dispatchEvent(second);
    });
    expect(baseProps.onCancel).not.toHaveBeenCalled();
    expect(baseProps.onConfirm).not.toHaveBeenCalled();
    expect(second.defaultPrevented).toBe(true);
    await act(async () => {
      backdrop().dispatchEvent(new MouseEvent("mousedown", { bubbles: true, detail: 1 }));
    });
    expect(baseProps.onCancel).toHaveBeenCalledTimes(1);
  });

  it("kills a held Enter or Space instead of answering on auto-repeat", async () => {
    await renderDialog();
    const repeatEnter = new KeyboardEvent("keydown", {
      key: "Enter",
      repeat: true,
      bubbles: true,
      cancelable: true,
    });
    await act(async () => {
      cancelButton().dispatchEvent(repeatEnter);
    });
    expect(repeatEnter.defaultPrevented).toBe(true);
    const repeatSpace = new KeyboardEvent("keydown", {
      key: " ",
      repeat: true,
      bubbles: true,
      cancelable: true,
    });
    await act(async () => {
      cancelButton().dispatchEvent(repeatSpace);
    });
    expect(repeatSpace.defaultPrevented).toBe(true);
    expect(baseProps.onCancel).not.toHaveBeenCalled();
    expect(baseProps.onConfirm).not.toHaveBeenCalled();

    const firstEnter = new KeyboardEvent("keydown", {
      key: "Enter",
      repeat: false,
      bubbles: true,
      cancelable: true,
    });
    await act(async () => {
      cancelButton().dispatchEvent(firstEnter);
    });
    expect(firstEnter.defaultPrevented).toBe(false);
  });

  it("returns focus to the element that opened it, whichever way the ask ends", async () => {
    function Harness() {
      const [open, setOpen] = useState(false);
      return (
        <>
          <button type="button" data-testid="trigger" onClick={() => setOpen(true)}>
            tab
          </button>
          <ConfirmDialog
            {...baseProps}
            tone="danger"
            open={open}
            onConfirm={() => setOpen(false)}
            onCancel={() => setOpen(false)}
          />
        </>
      );
    }
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root.render(<Harness />);
    });
    const trigger = container.querySelector<HTMLButtonElement>("[data-testid='trigger']")!;
    // The opener holds focus while the ask stands — the chip's × does in a
    // browser; happy-dom's click() does not focus, so the test does it by hand.
    await act(async () => {
      trigger.focus();
      trigger.click();
    });
    expect(document.activeElement).toBe(cancelButton());

    // Cancelled: the trigger takes focus back.
    await act(async () => {
      cancelButton().click();
    });
    expect(document.activeElement).toBe(trigger);

    // Confirmed: the same restore runs.
    await act(async () => {
      trigger.focus();
      trigger.click();
    });
    await act(async () => {
      confirmButton().click();
    });
    expect(document.activeElement).toBe(trigger);
  });
});

describe("the tall ask", () => {
  it("the card is capped against the viewport and the body scrolls inside it", async () => {
    // happy-dom does no layout, so a tall body cannot be measured here: this
    // pins the declarations the cap depends on instead.
    const css = readFileSync(resolve(import.meta.dirname, "ConfirmDialog.css"), "utf8");
    const card = css.match(/\.confirm-dialog\s*\{([^}]*)\}/)?.[1] ?? "";
    const body = css.match(/\.confirm-dialog-body\s*\{([^}]*)\}/)?.[1] ?? "";
    expect(card).toContain("calc(100vh");
    expect(card).toContain("flex-direction: column");
    expect(body).toContain("min-height: 0");
    expect(body).toContain("overflow-y: auto");
  });
});

function luminance(hex: string): number {
  const channels = [1, 3, 5].map((at) => {
    const channel = parseInt(hex.slice(at, at + 2), 16) / 255;
    return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
}

/** WCAG contrast ratio of two `#rrggbb` colours. */
function contrastRatio(a: string, b: string): number {
  const [hi, lo] = luminance(a) > luminance(b) ? [a, b] : [b, a];
  return (luminance(hi) + 0.05) / (luminance(lo) + 0.05);
}

/** The Cancel border's token, or a loud refusal. The guard compares the painted
    colour, so anything but one bare var() — a color-mix, a literal, two vars —
    fails here instead of resolving to the wrong colour. */
function cancelBorderToken(ruleBody: string): string {
  const longhand = ruleBody.match(/border-color\s*:\s*([^;]+)/)?.[1]?.trim();
  if (longhand !== undefined) {
    const token = longhand.match(/^var\((--[a-z-]+)\)$/)?.[1];
    if (token === undefined)
      throw new Error(`cancel border colour is not one bare var(): ${longhand}`);
    return token;
  }
  const shorthand = ruleBody.match(/border\s*:\s*([^;]+)/)?.[1]?.trim();
  if (shorthand === undefined) throw new Error("cancel border declaration not found");
  const token = shorthand.match(/^\S+\s+\S+\s+var\((--[a-z-]+)\)$/)?.[1];
  if (token === undefined)
    throw new Error(`cancel border colour is not one bare var(): ${shorthand}`);
  return token;
}

describe("the Cancel border", () => {
  it("refuses anything but one bare var() instead of guessing", () => {
    expect(cancelBorderToken("border: 1px solid var(--muted)")).toBe("--muted");
    expect(cancelBorderToken("border-color: var(--muted)")).toBe("--muted");
    expect(() =>
      cancelBorderToken("border: 1px solid color-mix(in srgb, var(--ink) 34%, transparent)"),
    ).toThrow("not one bare var()");
    expect(() => cancelBorderToken("border: 1px solid #1c1a17")).toThrow("not one bare var()");
    expect(() => cancelBorderToken("border: 1px solid var(--a) var(--b)")).toThrow(
      "not one bare var()",
    );
  });

  it("clears 3:1 non-text contrast against the card in both themes", () => {
    // No layout involved: the border token is read from the dialog's own
    // rule and resolved per theme through the assembled sheets.
    const sheets = [
      readFileSync(resolve(import.meta.dirname, "../styles/tokens.css"), "utf8"),
      readFileSync(resolve(import.meta.dirname, "ConfirmDialog.css"), "utf8"),
    ];
    const bodies = [
      ...sheets[1]!.matchAll(/\.confirm-dialog-cancel\s*(?::[^{]*)?\{([^}]*)\}/g),
    ].map((match) => match[1]!);
    const bordered = bodies.find((body) => /border(-color)?\s*:/.test(body));
    if (bordered === undefined) throw new Error("cancel border rule not found");
    const token = cancelBorderToken(bordered);
    for (const theme of ["light", "dark"] as const) {
      const proof = assembleCssProof(sheets, theme);
      const border = proof.token(token);
      const card = proof.token("--panel-card");
      expect(border, `${token} missing in ${theme}`).toMatch(/^#[0-9a-fA-F]{6}$/);
      expect(card, `--panel-card missing in ${theme}`).toMatch(/^#[0-9a-fA-F]{6}$/);
      const ratio = contrastRatio(border!, card!);
      expect(ratio, `${token} ${border} on --panel-card ${card} (${theme})`).toBeGreaterThanOrEqual(
        3,
      );
    }
  });
});
