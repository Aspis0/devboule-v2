// @vitest-environment happy-dom

// A scrim press arms nothing destructive: a clean form closes, a dirty form asks,
// and only the explicit Discard closes it.

import { act } from "react";
import { NewProjectDialog } from "../components/NewProjectDialog";
import { ProfileDialog } from "../features/settings/profiles/ProfileDialog";
import { cleanupModalsDom, mount, resetModalsStore } from "./modals-over-crescent.harness";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../features/plugins/install", () => ({ chooseAndInstall: vi.fn() }));
vi.mock("../features/design/DesignHistoryList", () => ({ DesignHistoryList: () => null }));
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

beforeEach(() => {
  resetModalsStore();
});

afterEach(() => {
  cleanupModalsDom();
});
describe("a scrim press arms nothing destructive", () => {
  function pressScrim(scrim: HTMLElement): void {
    scrim.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, button: 0 }));
  }

  it("a clean profile closes on a scrim press — the cancel gesture", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <ProfileDialog open title="Edit profile" busy={false} onClose={onClose}>
        {() => (
          <form>
            <input aria-label="Profile name" />
          </form>
        )}
      </ProfileDialog>,
    );

    const scrim = container.querySelector<HTMLElement>(".edit-scrim");
    if (scrim === null) throw new Error("profile scrim missing");
    await act(async () => pressScrim(scrim));
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
  });

  it("a dirty profile arms the discard confirm on a scrim press — and only the explicit Discard closes it", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <ProfileDialog open title="Edit profile" busy={false} onClose={onClose}>
        {({ markDirty }) => (
          <form>
            <input aria-label="Profile name" />
            <button type="button" onClick={markDirty}>
              Mark dirty
            </button>
          </form>
        )}
      </ProfileDialog>,
    );

    const dirty = container.querySelector<HTMLButtonElement>("button:not(.profile-dialog-close)");
    if (dirty === null) throw new Error("mark-dirty button missing");
    await act(async () => dirty.click());

    const scrim = container.querySelector<HTMLElement>(".edit-scrim");
    if (scrim === null) throw new Error("profile scrim missing");
    await act(async () => pressScrim(scrim));

    // The press armed a confirm: the dialog is still standing, nothing was
    // discarded, and the confirm says so itself.
    const confirm = container.querySelector<HTMLElement>(".device-inline-confirm");
    if (confirm === null) throw new Error("discard confirm did not arm");
    expect(onClose).not.toHaveBeenCalled();
    expect(container.querySelector(".edit-card")).not.toBeNull();

    const keepEditing = [...confirm.querySelectorAll("button")].find((button) =>
      button.textContent?.includes("Keep editing"),
    );
    if (keepEditing === undefined) throw new Error("keep-editing button missing");
    await act(async () => keepEditing.click());
    expect(container.querySelector(".device-inline-confirm")).toBeNull();

    // A second press re-arms it; only the explicit Discard closes the dialog.
    await act(async () => pressScrim(scrim));
    const confirmAgain = container.querySelector<HTMLElement>(".device-inline-confirm");
    if (confirmAgain === null) throw new Error("discard confirm did not re-arm");
    const discard = [...confirmAgain.querySelectorAll("button")].find((button) =>
      button.textContent?.includes("Discard"),
    );
    if (discard === undefined) throw new Error("discard button missing");
    await act(async () => discard.click());
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
  });

  it("the new-project backdrop press is a plain cancel", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <NewProjectDialog open onClose={onClose} onCreate={() => undefined} />,
    );

    const backdrop = container.querySelector<HTMLElement>(".workspace-project-dialog-backdrop");
    if (backdrop === null) throw new Error("new project backdrop missing");
    await act(async () => pressScrim(backdrop));
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
  });
});
