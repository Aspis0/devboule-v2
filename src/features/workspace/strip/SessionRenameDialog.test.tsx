// @vitest-environment happy-dom

// The rename dialog: the current name
// pre-filled and selected, Enter saves (the exact sessionSetName call),
// Escape cancels, the daemon's refusal next to the field with the draft
// kept, and focus back to whatever opened it.

import { act, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock("../../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/tauri")>();
  return { ...actual, sessionSetName: invokeMock };
});

import { sessionSetName } from "../../../lib/tauri";
import { getFocusableElements } from "../../../lib/focusableElements";
import { useAppStore } from "../../../store/appStore";
import { SessionRenameDialog } from "./SessionRenameDialog";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

interface RenameState {
  sessionId: string;
  title: string;
}

function renderProbe() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  const store: { setRename: (next: RenameState | null) => void } = {
    setRename: () => undefined,
  };
  function Probe() {
    const [rename, setRename] = useState<RenameState | null>(null);
    useEffect(() => {
      store.setRename = setRename;
    }, [setRename]);
    return (
      <>
        <button type="button" className="trigger">
          trigger
        </button>
        <SessionRenameDialog rename={rename} onClose={() => setRename(null)} />
      </>
    );
  }
  return {
    host,
    store,
    async mount() {
      await act(async () => {
        root.render(<Probe />);
      });
    },
    async unmount() {
      await act(async () => {
        root.unmount();
      });
    },
  };
}

function dialog(): HTMLElement {
  const found = document.querySelector<HTMLElement>('[role="dialog"]');
  if (found === null) throw new Error("rename dialog did not render");
  return found;
}

function input(): HTMLInputElement {
  const found = dialog().querySelector<HTMLInputElement>("input");
  if (found === null) throw new Error("rename input did not render");
  return found;
}

function typeIn(field: HTMLInputElement, value: string): void {
  const setValue = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
  if (setValue === undefined) throw new Error("input value setter did not exist");
  setValue.call(field, value);
  field.dispatchEvent(new Event("input", { bubbles: true }));
}

function saveButton(): HTMLButtonElement {
  const found = [...dialog().querySelectorAll<HTMLButtonElement>("button")].find(
    (button) => button.textContent === "Rename",
  );
  if (found === undefined) throw new Error("Rename button did not render");
  return found;
}

function pressKey(field: HTMLElement, key: string, init: KeyboardEventInit = {}): void {
  field.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...init }),
  );
}

afterEach(() => {
  document.body.replaceChildren();
  // A test that fails mid-body leaves its host mounted, and the shell's
  // modal token with it: the store is a singleton, so clear it by hand.
  useAppStore.setState({ modalOpenTokens: new Set() });
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(undefined);
});

describe("SessionRenameDialog", () => {
  it("renders nothing when closed", async () => {
    const probe = renderProbe();
    await probe.mount();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await probe.unmount();
  });

  it("opens with the current name pre-filled, selected, and focused", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    expect(input().value).toBe("worker one");
    expect(document.activeElement).toBe(input());
    expect(input().selectionStart).toBe(0);
    expect(input().selectionEnd).toBe("worker one".length);
    await probe.unmount();
  });

  it("registers with the shell while open", async () => {
    const probe = renderProbe();
    await probe.mount();
    expect(useAppStore.getState().modalOpenTokens.size).toBe(0);
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    expect(useAppStore.getState().modalOpenTokens.size).toBeGreaterThan(0);
    await probe.unmount();
  });

  it("Enter saves with the exact sessionSetName call and closes", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).toHaveBeenCalledTimes(1);
    expect(sessionSetName).toHaveBeenCalledWith("s.4242.7", "worker two");
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await probe.unmount();
  });

  it("saves the trimmed value — the daemon stores the trimmed value", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "  worker two  ");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).toHaveBeenCalledWith("s.4242.7", "worker two");
    await probe.unmount();
  });

  it("Escape cancels with no call", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Escape");
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await probe.unmount();
  });

  // Enter and Escape during an IME composition belong to the candidate
  // list: neither may reach the save, and the typed name stays untouched.
  it("leaves Enter and Escape to an open IME composition", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");

    await act(async () => {
      pressKey(input(), "Enter", { isComposing: true });
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(input().value).toBe("worker two");

    // Older engines report the composition commit as keyCode 229 alone.
    await act(async () => {
      pressKey(input(), "Enter", { keyCode: 229 });
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(input().value).toBe("worker two");

    await act(async () => {
      pressKey(input(), "Escape", { isComposing: true });
    });
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    expect(input().value).toBe("worker two");

    // Composition closed: the next Enter saves, as it always has.
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).toHaveBeenCalledTimes(1);
    expect(sessionSetName).toHaveBeenCalledWith("s.4242.7", "worker two");
    await probe.unmount();
  });

  // The composition guard may swallow Escape, never the card's Tab trap.
  it("keeps its Tab trap during a composition", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    const card = document.querySelector<HTMLElement>('[role="dialog"]');
    if (card === null) throw new Error("rename dialog missing");
    const focusable = getFocusableElements(card);
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (first === undefined || last === undefined || first === last) {
      throw new Error("rename dialog needs at least two focusables");
    }
    await act(async () => {
      last.focus();
    });

    const event = new KeyboardEvent("keydown", {
      key: "Tab",
      bubbles: true,
      cancelable: true,
      isComposing: true,
    });
    await act(async () => {
      last.dispatchEvent(event);
    });
    expect(event.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(first);
    await probe.unmount();
  });

  it("refuses an empty name with a sentence and keeps the dialog open", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(dialog().textContent).toContain("A session display name is required; it was empty.");
    expect(dialog().querySelector('[role="alert"]')).not.toBeNull();
    await probe.unmount();
  });

  it("refuses a whitespace-only name with the same sentence", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "    ");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(dialog().textContent).toContain("A session display name is required; it was empty.");
    await probe.unmount();
  });

  it("refuses a name past the daemon's limit, mirroring its sentence", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "x".repeat(61));
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(dialog().textContent).toContain(
      "A session display name is 61 characters; the limit is 60.",
    );
    await probe.unmount();
  });

  it("refuses a name with an invisible character, mirroring the daemon's sentence", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "wor\u{200b}ker");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(dialog().textContent).toContain(
      "A session display name must not contain an invisible formatting character.",
    );
    await probe.unmount();
  });

  it("shows a daemon refusal verbatim and keeps the draft", async () => {
    // A journal-door refusal: the daemon's rename road fails when the row
    // write fails, and the client mirror cannot predict it.
    invokeMock.mockRejectedValue({
      code: "journal",
      message: "journal is unavailable: the store is locked",
    });
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).toHaveBeenCalledTimes(1);
    expect(dialog().textContent).toContain("journal is unavailable: the store is locked");
    expect(input().value).toBe("worker two");
    await probe.unmount();
  });

  it("maps a non-command refusal through the app's error sentence", async () => {
    invokeMock.mockRejectedValue(new Error("pipe broken"));
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    // The sentence is the cause's own message — the case's whole point.
    expect(dialog().textContent).toContain("pipe broken");
    expect(dialog().querySelector('[role="alert"]')).not.toBeNull();
    expect(input().value).toBe("worker two");
    await probe.unmount();
  });

  it("shows the mapped sentence for a daemon refusal, with its words in the detail only", async () => {
    invokeMock.mockRejectedValue({
      code: "journal",
      message: "journal is unavailable: the store is locked",
    });
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    const alert = dialog().querySelector('[role="alert"]');
    expect(alert?.textContent).toContain("Saved history could not be read or written.");
    expect(alert?.querySelector(".error-detail-sr-only")?.textContent).toBe(
      "journal is unavailable: the store is locked",
    );
    expect(input().value).toBe("worker two");
    await probe.unmount();
  });

  it("returns focus to the trigger after a save", async () => {
    const probe = renderProbe();
    await probe.mount();
    const trigger = probe.host.querySelector<HTMLButtonElement>(".trigger");
    if (trigger === null) throw new Error("trigger did not render");
    await act(async () => {
      trigger.focus();
    });
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    expect(document.activeElement).toBe(input());
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(document.activeElement).toBe(trigger);
    await probe.unmount();
  });

  it("returns focus to the trigger after an Escape", async () => {
    const probe = renderProbe();
    await probe.mount();
    const trigger = probe.host.querySelector<HTMLButtonElement>(".trigger");
    if (trigger === null) throw new Error("trigger did not render");
    await act(async () => {
      trigger.focus();
    });
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    await act(async () => {
      pressKey(input(), "Escape");
    });
    expect(document.activeElement).toBe(trigger);
    await probe.unmount();
  });

  it("shows the refusal on open when the pre-fill is a name the daemon's door refuses", async () => {
    // The daemon's own title derivation keeps U+200C/U+200D, so an
    // auto-titled session can carry a pre-fill the rename validator
    // refuses. The sentence must be on screen the moment the dialog opens —
    // a grey button and no message is the one outcome this must never be.
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "family 👨‍👩‍👧" });
    await act(async () => {});
    expect(dialog().textContent).toContain(
      "A session display name must not contain an invisible formatting character.",
    );
    expect(input().getAttribute("aria-invalid")).toBe("true");
    expect(saveButton().disabled).toBe(true);
    // And no call has been made: the refusal is a client-side sentence, not a
    // daemon round trip.
    expect(sessionSetName).not.toHaveBeenCalled();
    await probe.unmount();
  });

  it("a title change resyncs an untouched draft but keeps the user's edit", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "Agent s.4242.7" });
    await act(async () => {});
    // Untouched: the auto-title landing moves the field with the tab.
    probe.store.setRename({ sessionId: "s.4242.7", title: "run the flaky test suite" });
    await act(async () => {});
    expect(input().value).toBe("run the flaky test suite");
    // Edited: the user's words outrank the roster behind the dialog.
    typeIn(input(), "ci-watcher");
    await act(async () => {});
    probe.store.setRename({ sessionId: "s.4242.7", title: "run the flaky test suite" });
    await act(async () => {});
    expect(input().value).toBe("ci-watcher");
    await probe.unmount();
  });

  it("a roster push landing mid-save keeps the lock and closes exactly once", async () => {
    // The daemon writes the record and pushes the roster before (or after)
    // the RPC reply — the two are unordered. If the push lands while the call
    // is on the wire, the dialog must hold the lock: every exit stays dead
    // until the reply closes it, exactly once.
    let resolveCall: (() => void) | null = null;
    invokeMock.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveCall = resolve;
        }),
    );
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).toHaveBeenCalledTimes(1);

    // The auto-title landing re-keys the dialog's target mid-save.
    probe.store.setRename({ sessionId: "s.4242.7", title: "run the flaky test suite" });
    await act(async () => {});

    // The lock holds: the field and every exit are inert.
    expect(input().disabled).toBe(true);
    await act(async () => {
      pressKey(input(), "Escape");
    });
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    const cancel = [...dialog().querySelectorAll<HTMLButtonElement>("button")].find(
      (button) => button.textContent === "Cancel",
    );
    expect(cancel?.disabled).toBe(true);

    // The reply settles the save and closes the dialog — on the reply, not on
    // the push.
    await act(async () => {
      resolveCall?.();
    });
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await probe.unmount();
  });

  it("a refusal does not survive into the next open", async () => {
    // The dialog stays mounted between opens, so a refusal the user never
    // cleared must die with the session it was about — not greet the next one
    // with a sentence about a name it never saw.
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "\u{200d}worker");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(dialog().textContent).toContain(
      "A session display name must not contain an invisible formatting character.",
    );
    // Escape closes; the dialog stays mounted.
    await act(async () => {
      pressKey(input(), "Escape");
    });
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    // A different agent, a valid name — and no trace of the first refusal.
    probe.store.setRename({ sessionId: "s.4242.8", title: "worker two" });
    await act(async () => {});
    expect(dialog().textContent).not.toContain(
      "A session display name must not contain an invisible formatting character.",
    );
    expect(input().getAttribute("aria-invalid")).toBe("false");
    await probe.unmount();
  });

  it("Enter on an unchanged name makes no call", async () => {
    // The daemon's store computes `changed` and skips the
    // roster push, so the cost of skipping this is one IPC that changes nothing.
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).not.toHaveBeenCalled();
    expect(document.querySelector('[role="dialog"]')).not.toBeNull();
    await probe.unmount();
  });

  it("two Enters in one press make one call", async () => {
    // The input's keydown and the form's implicit submission both reach the
    // handler in one press; the synchronous ref makes the second a no-op.
    let resolveCall: (() => void) | null = null;
    invokeMock.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveCall = resolve;
        }),
    );
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).toHaveBeenCalledTimes(1);
    await act(async () => {
      resolveCall?.();
    });
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    await probe.unmount();
  });

  it("a save that resolves leaves the next save able to fire", async () => {
    // The finally resets both halves of the submitting flag: without the ref
    // half, a second save after a first would be a silent no-op.
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    typeIn(input(), "worker two");
    await act(async () => {
      pressKey(input(), "Enter");
    });
    expect(sessionSetName).toHaveBeenCalledTimes(1);
    expect(document.querySelector('[role="dialog"]')).toBeNull();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker two" });
    await act(async () => {});
    typeIn(input(), "worker three");
    await act(async () => {
      saveButton().click();
    });
    expect(sessionSetName).toHaveBeenCalledTimes(2);
    expect(sessionSetName).toHaveBeenLastCalledWith("s.4242.7", "worker three");
    await probe.unmount();
  });

  it("disables Rename while the name is unchanged", async () => {
    const probe = renderProbe();
    await probe.mount();
    probe.store.setRename({ sessionId: "s.4242.7", title: "worker one" });
    await act(async () => {});
    expect(saveButton().disabled).toBe(true);
    typeIn(input(), "worker two");
    await act(async () => {});
    expect(saveButton().disabled).toBe(false);
    await probe.unmount();
  });
});
