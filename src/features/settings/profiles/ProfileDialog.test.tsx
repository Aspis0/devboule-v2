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
  }: {
    onClose?: () => void;
    withDirtyButton?: boolean;
  } = {}) {
    root = createRoot(container);
    act(() => {
      root!.render(
        <ProfileDialog title="Edit profile — Coder" onClose={onClose}>
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
    expect(dialog?.textContent).toContain("Edit profile — Coder");
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
    cancel.focus();
    pressKey("Tab");
    expect(document.activeElement).toBe(first);
    pressKey("Tab", true);
    expect(document.activeElement).toBe(cancel);
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
});
