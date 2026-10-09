// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { SettingsDialog, SettingsRow, SettingsSection, ByteCounter } from "./rows";

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

describe("shared Settings section heading", () => {
  it("renders the label as a heading and names its section after it", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <SettingsSection label="Daemon">
          <SettingsRow title="Version" control={<span>0.1.0</span>} />
        </SettingsSection>,
      );
    });
    const heading = container.querySelector("h3");
    expect(heading?.textContent).toBe("Daemon");
    const section = container.querySelector("[data-settings-section]");
    const labelId = section?.getAttribute("aria-labelledby");
    expect(labelId).toBe(heading?.id);
    expect(labelId).toBeTruthy();
    expect(document.getElementById(labelId!)?.textContent).toBe("Daemon");
  });
});

describe("shared Settings row pattern", () => {
  it("renders a title, at most one short description, and the control on the right", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <SettingsRow
          title="Standing instructions"
          description="Sent with every agent"
          control={<button type="button">Edit</button>}
        />,
      );
    });
    const row = container.querySelector("[data-settings-row]");
    if (!row) throw new Error("settings row did not render");
    expect(row.textContent).toContain("Standing instructions");
    expect(row.querySelector("button")?.textContent).toBe("Edit");
    // One description line at most, never a paragraph of explanation.
    expect(row.querySelectorAll("p").length).toBe(0);
    expect(row.querySelectorAll("[data-settings-row-description]").length).toBeLessThanOrEqual(1);
  });

  it("renders a section label with its action on the right", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(
        <SettingsSection
          label="Agent profiles"
          action={
            <button type="button" aria-label="New profile">
              +
            </button>
          }
        >
          <div>rows</div>
        </SettingsSection>,
      );
    });
    const section = container.querySelector("[data-settings-section]");
    if (!section) throw new Error("settings section did not render");
    expect(section.textContent).toContain("Agent profiles");
    expect(section.querySelector('button[aria-label="New profile"]')).not.toBeNull();
  });
});

describe("byte counter", () => {
  it("stays hidden until the last tenth of the cap", async () => {
    root = createRoot(container);
    await act(async () => {
      root!.render(<ByteCounter bytes={899} cap={1000} />);
    });
    expect(container.textContent).not.toContain("899");
    await act(async () => {
      root!.render(<ByteCounter bytes={900} cap={1000} />);
    });
    expect(container.textContent).toContain("900 / 1000 bytes");
  });
});

describe("shared Settings dialog", () => {
  function renderDialog(options: {
    dirty: boolean;
    onClose: () => void;
    children?: React.ReactNode;
  }) {
    const { dirty, onClose, children } = options;
    root = createRoot(container);
    return act(async () => {
      root!.render(
        <SettingsDialog open title="Standing instructions" dirty={dirty} onClose={onClose}>
          {({ requestClose }) => (
            <>
              {children ?? <input aria-label="Instructions" />}
              <button type="button" onClick={requestClose}>
                Cancel
              </button>
            </>
          )}
        </SettingsDialog>,
      );
    });
  }

  it("closes at once on Cancel when nothing changed", async () => {
    const onClose = vi.fn();
    await renderDialog({ dirty: false, onClose });
    const cancel = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Cancel",
    );
    if (!cancel) throw new Error("Cancel did not render");
    await act(async () => cancel.click());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows a discard step inside the dialog on Cancel with unsaved changes", async () => {
    const onClose = vi.fn();
    await renderDialog({ dirty: true, onClose });
    const cancel = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Cancel",
    );
    if (!cancel) throw new Error("Cancel did not render");
    await act(async () => cancel.click());
    // A visible outcome where the person is: the dialog becomes the confirm
    // step instead of closing silently.
    expect(onClose).not.toHaveBeenCalled();
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog?.textContent).toContain("Discard unsaved changes?");
    const discard = Array.from(dialog?.querySelectorAll("button") ?? []).find(
      (button) => button.textContent === "Discard",
    );
    if (!discard) throw new Error("Discard step did not render its Discard button");
    await act(async () => discard.click());
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("shows the same discard step on Escape with unsaved changes", async () => {
    const onClose = vi.fn();
    await renderDialog({ dirty: true, onClose });
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(onClose).not.toHaveBeenCalled();
    expect(container.querySelector('[role="dialog"]')?.textContent).toContain(
      "Discard unsaved changes?",
    );
  });

  it("closes at once on Escape when nothing changed", async () => {
    const onClose = vi.fn();
    await renderDialog({ dirty: false, onClose });
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
