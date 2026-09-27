// @vitest-environment happy-dom

// The profile dialog shell: scrim, focus trap, Escape, and the dirty check.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ProfileDialog } from "./ProfileDialog";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("ProfileDialog", () => {
  let container: HTMLDivElement;
  let root: Root | undefined;

  function renderDialog({
    onClose = () => undefined,
    withDirtyButton = false,
    busy = false,
  }: {
    onClose?: () => void;
    withDirtyButton?: boolean;
    busy?: boolean;
  } = {}) {
    root = createRoot(container);
    act(() => {
      root!.render(
        <ProfileDialog title="Edit profile — Coder" busy={busy} onClose={onClose}>
          {({ requestClose, markDirty }) => (
            <>
              <input aria-label="First field" />
              <button type="button" onClick={requestClose}>
                Cancel
              </button>
              {withDirtyButton ? (
                <button type="button" onClick={markDirty}>
                  Make dirty
                </button>
              ) : null}
            </>
          )}
        </ProfileDialog>,
      );
    });
  }

  function pressKey(key: string, shiftKey = false) {
    act(() => {
      document.dispatchEvent(
        new KeyboardEvent("keydown", { key, shiftKey, bubbles: true, cancelable: true }),
      );
    });
  }

  function field(label: string): HTMLElement {
    const el = container.querySelector<HTMLElement>(`[aria-label="${label}"]`);
    if (!el) throw new Error(`control ${label} did not render`);
    return el;
  }

  function buttonByText(text: string): HTMLElement {
    const el = Array.from(container.querySelectorAll<HTMLElement>("button")).find(
      (candidate) => candidate.textContent === text,
    );
    if (!el) throw new Error(`button ${text} did not render`);
    return el;
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(() => {
    if (root !== undefined) {
      const current = root;
      act(() => current.unmount());
    }
    container.remove();
  });

  it("renders on the scrim as a modal dialog with its title", () => {
    renderDialog();
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog?.getAttribute("aria-modal")).toBe("true");
    // The labelled-by id names the visible title: a typo in the wiring
    // would leave the dialog unnamed and every other assertion green.
    const labelledBy = dialog?.getAttribute("aria-labelledby") ?? "";
    expect(labelledBy).not.toBe("");
    expect(container.querySelector(`#${labelledBy}`)?.textContent).toBe("Edit profile — Coder");
    expect(container.querySelector(".edit-scrim")).not.toBeNull();
    expect(container.querySelector(".edit-card")).not.toBeNull();
  });

  it("moves focus to the first field on open", () => {
    renderDialog();
    expect(document.activeElement).toBe(field("First field"));
  });

  it("traps Tab inside the dialog", () => {
    renderDialog();
    const first = field("First field");
    const cancel = buttonByText("Cancel");
    const close = container.querySelector<HTMLElement>('button[aria-label="Close profile dialog"]');
    if (!close) throw new Error("dialog × button did not render");
    // DOM order is ×, field, Cancel: Tab wraps at both ends.
    cancel.focus();
    pressKey("Tab");
    expect(document.activeElement).toBe(close);
    pressKey("Tab", true);
    expect(document.activeElement).toBe(cancel);
    close.focus();
    pressKey("Tab", true);
    expect(document.activeElement).toBe(cancel);
    expect(first).not.toBeNull();
  });

  it("closes on Escape when nothing changed", () => {
    const onClose = vi.fn();
    renderDialog({ onClose });
    pressKey("Escape");
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("holds Escape behind a discard check when the form is dirty", () => {
    const onClose = vi.fn();
    renderDialog({ onClose, withDirtyButton: true });
    act(() => buttonByText("Make dirty").click());
    pressKey("Escape");
    expect(onClose).not.toHaveBeenCalled();
    expect(container.textContent).toContain("Discard unsaved changes?");
  });

  it("discards on Discard and keeps editing on Keep editing", () => {
    const onClose = vi.fn();
    renderDialog({ onClose, withDirtyButton: true });
    act(() => buttonByText("Make dirty").click());
    pressKey("Escape");
    act(() => buttonByText("Keep editing").click());
    expect(onClose).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("Discard unsaved changes?");
    pressKey("Escape");
    act(() => buttonByText("Discard").click());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("routes Cancel through the same dirty check", () => {
    const onClose = vi.fn();
    renderDialog({ onClose, withDirtyButton: true });
    act(() => buttonByText("Cancel").click());
    expect(onClose).toHaveBeenCalledTimes(1);
    onClose.mockClear();
    act(() => buttonByText("Make dirty").click());
    act(() => buttonByText("Cancel").click());
    expect(onClose).not.toHaveBeenCalled();
    expect(container.textContent).toContain("Discard unsaved changes?");
  });

  it("closes a clean dialog through the × button", () => {
    const onClose = vi.fn();
    renderDialog({ onClose });
    const close = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Close profile dialog"]',
    );
    if (!close) throw new Error("dialog × button did not render");
    act(() => close.click());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes on a left scrim click, never on a card or right click", () => {
    const onClose = vi.fn();
    renderDialog({ onClose });
    const scrim = container.querySelector<HTMLElement>(".edit-scrim");
    const card = container.querySelector<HTMLElement>(".edit-card");
    if (!scrim || !card) throw new Error("scrim or card did not render");
    // Inside the card: the target is not the scrim, so nothing closes.
    act(() => {
      card.dispatchEvent(new MouseEvent("mousedown", { button: 0, bubbles: true }));
    });
    expect(onClose).not.toHaveBeenCalled();
    // Right button on the scrim: not a dismissal.
    act(() => {
      scrim.dispatchEvent(new MouseEvent("mousedown", { button: 2, bubbles: true }));
    });
    expect(onClose).not.toHaveBeenCalled();
    // Left button on the scrim itself: close.
    act(() => {
      scrim.dispatchEvent(new MouseEvent("mousedown", { button: 0, bubbles: true }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("holds every exit while a save is in flight", () => {
    const onClose = vi.fn();
    renderDialog({ onClose, withDirtyButton: true, busy: true });
    // Dirty or not, nothing closes mid-save: Escape, scrim, ×, Cancel.
    pressKey("Escape");
    expect(onClose).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("Discard unsaved changes?");
    const scrim = container.querySelector<HTMLElement>(".edit-scrim");
    act(() => {
      scrim?.dispatchEvent(new MouseEvent("mousedown", { button: 0, bubbles: true }));
    });
    expect(onClose).not.toHaveBeenCalled();
    act(() => buttonByText("Cancel").click());
    expect(onClose).not.toHaveBeenCalled();
    // The discard arm itself is unreachable, and Discard is dead if armed.
    act(() => buttonByText("Make dirty").click());
    pressKey("Escape");
    expect(container.textContent).not.toContain("Discard unsaved changes?");
  });
});
