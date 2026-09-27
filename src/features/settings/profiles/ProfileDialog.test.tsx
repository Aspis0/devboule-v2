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
  let currentProps: { onClose: () => void; withDirtyButton: boolean; busy: boolean } = {
    onClose: () => undefined,
    withDirtyButton: false,
    busy: false,
  };

  function renderDialogBody(open: boolean) {
    return (
      <ProfileDialog
        open={open}
        title="Edit profile — Coder"
        busy={currentProps.busy}
        onClose={currentProps.onClose}
      >
        {({ requestClose, markDirty }) => (
          <>
            <input aria-label="First field" />
            <button type="button" onClick={requestClose}>
              Cancel
            </button>
            {currentProps.withDirtyButton ? (
              <button type="button" onClick={markDirty}>
                Make dirty
              </button>
            ) : null}
          </>
        )}
      </ProfileDialog>
    );
  }

  function renderDialog({
    onClose = () => undefined,
    withDirtyButton = false,
    busy = false,
  }: {
    onClose?: () => void;
    withDirtyButton?: boolean;
    busy?: boolean;
  } = {}) {
    currentProps = { onClose, withDirtyButton, busy };
    root = createRoot(container);
    act(() => {
      root!.render(renderDialogBody(true));
    });
  }

  /** The dialog is always mounted: re-render it with a different open. */
  function setOpen(open: boolean) {
    act(() => {
      root!.render(renderDialogBody(open));
    });
  }

  // Re-render with the save started: the busy transition mid-dialog.
  function renderDialogHandle({
    onClose = () => undefined,
    withDirtyButton = false,
  }: {
    onClose?: () => void;
    withDirtyButton?: boolean;
  } = {}) {
    renderDialog({ onClose, withDirtyButton, busy: false });
    return {
      rerenderBusy() {
        currentProps.busy = true;
        act(() => {
          root!.render(renderDialogBody(true));
        });
      },
    };
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
    // The dialog is always mounted: focus must move on the OPEN, not on
    // mount — while it is closed the pane behind is inert, so a mount-time
    // placement would be swallowed.
    root = createRoot(container);
    act(() => {
      root!.render(renderDialogBody(false));
    });
    expect(container.querySelector(".edit-card")).toBeNull();
    act(() => {
      root!.render(renderDialogBody(true));
    });
    expect(document.activeElement).toBe(field("First field"));
  });

  it("a fresh open does not inherit the last session's discard arm", () => {
    const onClose = vi.fn();
    renderDialog({ onClose, withDirtyButton: true });
    act(() => buttonByText("Make dirty").click());
    pressKey("Escape");
    expect(container.querySelector(".device-inline-confirm")).not.toBeNull();
    act(() => buttonByText("Discard").click());
    expect(onClose).toHaveBeenCalledTimes(1);
    // Close, then reopen: the arm must not survive into the next session,
    // and a clean form must close at once.
    setOpen(false);
    setOpen(true);
    expect(container.querySelector(".device-inline-confirm")).toBeNull();
    act(() => buttonByText("Cancel").click());
    expect(onClose).toHaveBeenCalledTimes(2);
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
    // Dirty or not, nothing closes outright mid-save: Escape, scrim, ×
    // and Cancel arm the honest exit instead.
    pressKey("Escape");
    expect(onClose).not.toHaveBeenCalled();
    // The sentence promises nothing about the outcome: the save continues,
    // and a refusal is named, not foretold.
    expect(container.textContent).toContain("A save is still running.");
    expect(container.textContent).toContain("the reason will appear on the page behind");
    expect(container.textContent).not.toContain("is being saved");
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

  it("lets a busy Escape abandon the view while the write finishes", () => {
    const onClose = vi.fn();
    renderDialog({ onClose, withDirtyButton: true, busy: true });
    act(() => buttonByText("Make dirty").click());
    pressKey("Escape");
    expect(onClose).not.toHaveBeenCalled();
    expect(container.textContent).toContain("A save is still running.");
    act(() => buttonByText("Close dialog").click());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("keeps waiting when told to", () => {
    const onClose = vi.fn();
    renderDialog({ onClose, withDirtyButton: true, busy: true });
    pressKey("Escape");
    expect(container.textContent).toContain("A save is still running.");
    act(() => buttonByText("Keep waiting").click());
    expect(onClose).not.toHaveBeenCalled();
    expect(container.textContent).not.toContain("A save is still running.");
    // The confirm unmounted under its focused button: focus stays in the
    // card, with the modal still open.
    expect(document.activeElement?.classList.contains("edit-card")).toBe(true);
  });

  it("hands the exit to the leaving confirm when a save starts under discard", () => {
    const onClose = vi.fn();
    const { rerenderBusy } = renderDialogHandle({ onClose, withDirtyButton: true });
    act(() => buttonByText("Make dirty").click());
    pressKey("Escape");
    expect(container.textContent).toContain("Discard unsaved changes?");
    // The save starts: discard disarms, Escape now arms leaving only.
    act(() => rerenderBusy());
    expect(container.textContent).not.toContain("Discard unsaved changes?");
    pressKey("Escape");
    expect(container.textContent).toContain("A save is still running.");
    expect(container.textContent).not.toContain("Discard unsaved changes?");
    expect(onClose).not.toHaveBeenCalled();
  });
});
