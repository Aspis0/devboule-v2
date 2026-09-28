// @vitest-environment happy-dom

// The confirm host: the promise shape both destructive surfaces ask
// through — true only on the affirmative, false on Cancel, Escape, the
// scrim, an unmount, a second ask, or a missing provider. happy-dom does
// no layout, so no geometry is asserted here.

import { act, StrictMode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ConfirmProvider, useConfirmAsk, type ConfirmAskFn } from "./ConfirmHost";
import { useAppStore } from "../store/appStore";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const FIRST = {
  title: "Discard changes",
  message: 'Discard every uncommitted change to "notes/todo.md"? This cannot be undone.',
  confirmLabel: "Discard",
};

const SECOND = {
  title: "Delete README.md",
  message: 'Delete the file "README.md"? This cannot be undone.',
  confirmLabel: "Delete",
};

let container: HTMLDivElement;
let root: Root | null;
const captured: { ask: ConfirmAskFn | null } = { ask: null };

function Probe() {
  const ask = useConfirmAsk();
  return <button type="button" data-testid="capture" onClick={() => void (captured.ask = ask)} />;
}

async function renderProvider(): Promise<void> {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  captured.ask = null;
  await act(async () => {
    root!.render(
      <ConfirmProvider>
        <Probe />
      </ConfirmProvider>,
    );
  });
  await act(async () => {
    container.querySelector<HTMLButtonElement>("[data-testid='capture']")!.click();
  });
  if (captured.ask === null) throw new Error("the probe did not capture the ask");
}

/** Start an ask and let the dialog stand; the answer lands in `box`. */
async function startAsk(box: { result?: boolean }): Promise<void> {
  const ask = captured.ask!;
  await act(async () => {
    void ask(FIRST).then((answer) => {
      box.result = answer;
    });
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

beforeEach(() => {
  useAppStore.setState({ modalOpenTokens: new Set() });
});

afterEach(async () => {
  if (root !== null) {
    await act(async () => {
      root!.unmount();
    });
    root = null;
  }
  container.remove();
  expect(document.querySelector(".confirm-dialog")).toBeNull();
  useAppStore.setState({ modalOpenTokens: new Set() });
});

describe("the ask it carries", () => {
  it("renders the title, the body and the act's label as the danger affirmative", async () => {
    await renderProvider();
    const box: { result?: boolean } = {};
    await startAsk(box);

    expect(dialog().querySelector(".confirm-dialog-title")?.textContent).toBe(FIRST.title);
    expect(dialog().querySelector(".confirm-dialog-body")?.textContent).toBe(FIRST.message);
    expect(confirmButton().textContent).toBe("Discard");
    expect(confirmButton().classList.contains("confirm-dialog-confirm-danger")).toBe(true);
    expect(cancelButton().textContent).toBe("Cancel");
    expect(document.activeElement).toBe(cancelButton());
    expect(useAppStore.getState().modalOpenTokens.size).toBe(1);

    await act(async () => {
      confirmButton().click();
    });
    expect(box.result).toBe(true);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
  });

  it("renders a caller-named safe answer when the ask names one", async () => {
    await renderProvider();
    const box: { result?: boolean } = {};
    await act(async () => {
      void captured.ask!({ ...FIRST, cancelLabel: "Keep them" }).then((answer) => {
        box.result = answer;
      });
    });
    expect(cancelButton().textContent).toBe("Keep them");
    await act(async () => {
      cancelButton().click();
    });
    expect(box.result).toBe(false);
  });
});

describe("the ways out that decline", () => {
  it("Cancel resolves false and closes the dialog", async () => {
    await renderProvider();
    const box: { result?: boolean } = {};
    await startAsk(box);

    await act(async () => {
      cancelButton().click();
    });
    expect(box.result).toBe(false);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  it("Escape resolves false and never confirms", async () => {
    await renderProvider();
    const box: { result?: boolean } = {};
    await startAsk(box);

    await act(async () => {
      confirmButton().dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(box.result).toBe(false);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  it("a press on the scrim resolves false", async () => {
    await renderProvider();
    const box: { result?: boolean } = {};
    await startAsk(box);

    await act(async () => {
      backdrop().dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    });
    expect(box.result).toBe(false);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });
});

describe("the standing ask's lifetime", () => {
  it("survives StrictMode's double effect: the ask stands until it is answered", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    captured.ask = null;
    await act(async () => {
      root!.render(
        <StrictMode>
          <ConfirmProvider>
            <Probe />
          </ConfirmProvider>
        </StrictMode>,
      );
    });
    await act(async () => {
      container.querySelector<HTMLButtonElement>("[data-testid='capture']")!.click();
    });
    if (captured.ask === null) throw new Error("the probe did not capture the ask");

    const box: { result?: boolean } = {};
    await startAsk(box);
    // The throwaway mount's cleanup must not have closed the host: the
    // dialog is open and the ask is still pending.
    expect(document.querySelector(".confirm-dialog")).not.toBeNull();
    expect(box.result).toBeUndefined();

    await act(async () => {
      confirmButton().click();
    });
    expect(box.result).toBe(true);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });
  it("unmounting while the dialog is open resolves false and releases the token", async () => {
    await renderProvider();
    const box: { result?: boolean } = {};
    await startAsk(box);
    expect(useAppStore.getState().modalOpenTokens.size).toBe(1);

    await act(async () => {
      root!.unmount();
    });
    root = null;
    expect(box.result).toBe(false);
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
  });

  // The affirmative is already given when the unmount lands between the
  // settle and the close's commit: the queued answer keeps what the
  // person chose instead of flipping it to a decline.
  it("unmounting with an answered ask keeps the affirmative", async () => {
    await renderProvider();
    const box: { result?: boolean } = {};
    await startAsk(box);

    await act(async () => {
      confirmButton().click();
      root!.unmount();
    });
    root = null;
    await act(async () => undefined);
    expect(box.result).toBe(true);
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
  });

  it("a second ask while one stands is declined at once and leaves the first standing", async () => {
    await renderProvider();
    const ask = captured.ask!;
    const first: { result?: boolean } = {};
    await act(async () => {
      void ask(FIRST).then((answer) => {
        first.result = answer;
      });
    });

    const second = await ask(SECOND);
    expect(second).toBe(false);
    expect(first.result).toBeUndefined();
    expect(dialog().querySelector(".confirm-dialog-title")?.textContent).toBe(FIRST.title);

    await act(async () => {
      confirmButton().click();
    });
    expect(first.result).toBe(true);
  });

  it("an ask after unmount resolves false instead of hanging", async () => {
    await renderProvider();
    const held = captured.ask!;
    await act(async () => {
      root!.unmount();
    });
    root = null;

    const answer = await held(FIRST);
    expect(answer).toBe(false);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });

  it("without a provider the ask declines and renders nothing", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    captured.ask = null;
    await act(async () => {
      root!.render(<Probe />);
    });
    await act(async () => {
      container.querySelector<HTMLButtonElement>("[data-testid='capture']")!.click();
    });

    const answer = await captured.ask!(FIRST);
    expect(answer).toBe(false);
    expect(document.querySelector(".confirm-dialog")).toBeNull();
  });
});
