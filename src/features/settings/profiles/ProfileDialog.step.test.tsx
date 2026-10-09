// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ProfileDialog } from "./ProfileDialog";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root | undefined;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  if (root !== undefined) await act(async () => root!.unmount());
  root = undefined;
  container.remove();
});

async function renderDialog(options: {
  dirtyOnMount: boolean;
  busy?: boolean;
  onClose?: () => void;
  press: "cancel" | "close" | "escape";
}) {
  const onClose = options.onClose ?? vi.fn();
  let markDirty: (() => void) | undefined;
  root = createRoot(container);
  await act(async () => {
    root!.render(
      <ProfileDialog open title="New profile" busy={options.busy ?? false} onClose={onClose}>
        {({ requestClose, markDirty: mark }) => {
          markDirty = mark;
          return (
            <>
              <input aria-label="Profile name" />
              <button type="button" onClick={requestClose}>
                Cancel
              </button>
            </>
          );
        }}
      </ProfileDialog>,
    );
  });
  if (options.dirtyOnMount) {
    await act(async () => markDirty!());
  }
  if (options.press === "cancel") {
    const cancel = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Cancel",
    );
    if (!cancel) throw new Error("Cancel did not render");
    await act(async () => cancel.click());
  } else if (options.press === "close") {
    const close = container.querySelector('button[aria-label="Close profile dialog"]');
    if (!close) throw new Error("dialog close did not render");
    await act(async () => (close as HTMLButtonElement).click());
  } else {
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
  }
  return onClose;
}

describe("profile dialog — Cancel, × and Escape always answer visibly", () => {
  it("Cancel with no edits closes at once", async () => {
    const onClose = await renderDialog({ dirtyOnMount: false, press: "cancel" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("Cancel with edits shows an inline discard confirm where the person is", async () => {
    const onClose = await renderDialog({ dirtyOnMount: true, press: "cancel" });
    expect(onClose).not.toHaveBeenCalled();
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain("Discard unsaved changes?");
    // The confirm lives in the dialog's own step, never at the top of a
    // scrolled form: the action buttons are beside it.
    const discard = Array.from(dialog?.querySelectorAll("button") ?? []).find(
      (button) => button.textContent === "Discard",
    );
    if (!discard) throw new Error("discard confirm did not render its Discard button");
    await act(async () => (discard as HTMLButtonElement).click());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("× with edits shows the same confirm step", async () => {
    const onClose = await renderDialog({ dirtyOnMount: true, press: "close" });
    expect(onClose).not.toHaveBeenCalled();
    expect(container.querySelector('[role="dialog"]')?.textContent).toContain(
      "Discard unsaved changes?",
    );
  });

  it("Escape with edits shows the same confirm step", async () => {
    const onClose = await renderDialog({ dirtyOnMount: true, press: "escape" });
    expect(onClose).not.toHaveBeenCalled();
    expect(container.querySelector('[role="dialog"]')?.textContent).toContain(
      "Discard unsaved changes?",
    );
  });

  it("Escape with no edits closes at once", async () => {
    const onClose = await renderDialog({ dirtyOnMount: false, press: "escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
