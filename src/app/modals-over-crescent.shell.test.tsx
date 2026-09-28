// @vitest-environment happy-dom

// The shell contract with a real modal inside it: the band stays shut, the surface
// does not switch, and a portaled dialog holds too — the defect's own configuration.

import { act } from "react";
import { NewProjectDialog } from "../components/NewProjectDialog";
import {
  ShellWith,
  ShellWithConfirmDialog,
  ShellWithProjectDialog,
  cleanupModalsDom,
  hoverBand,
  modalCount,
  mount,
  navIsOpen,
  resetModalsStore,
} from "./modals-over-crescent.harness";
import { useAppStore } from "../store/appStore";

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
describe("the shell with a modal open (real modal, real shell)", () => {
  it("hovering the band does not open the nav while a modal is open, and does again after it closes", async () => {
    const { container, root } = await mount(<ShellWithProjectDialog />);

    expect(modalCount()).toBe(1);
    expect(navIsOpen(container)).toBe(false);

    await hoverBand(container);
    expect(navIsOpen(container)).toBe(false);

    // The modal's own Escape still works while the nav is being kept shut.
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(modalCount()).toBe(0);

    await hoverBand(container);
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("a click on the line does not open the nav while a modal is open", async () => {
    const { container, root } = await mount(<ShellWithProjectDialog />);

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver did not render");
    await act(async () => sliver.click());
    expect(navIsOpen(container)).toBe(false);
    await act(async () => root.unmount());
  });

  it("the surface does not switch while a modal is open, and does after it closes", async () => {
    const { root } = await mount(<ShellWithProjectDialog />);

    await act(async () => {
      useAppStore.getState().selectSurface("polis");
    });
    expect(useAppStore.getState().activeSurface).toBe("workspace");

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(modalCount()).toBe(0);
    await act(async () => {
      useAppStore.getState().selectSurface("polis");
    });
    expect(useAppStore.getState().activeSurface).toBe("polis");
    await act(async () => root.unmount());
  });

  it("a portaled dialog holds the band shut too — the destructive confirm dialog", async () => {
    const onCancel = vi.fn();
    const { container, root } = await mount(<ShellWithConfirmDialog onCancel={onCancel} />);

    expect(modalCount()).toBe(1);
    await hoverBand(container);
    expect(navIsOpen(container)).toBe(false);

    // Its own Escape cancels the ask, and the band works again afterwards.
    const cancel = document.querySelector<HTMLButtonElement>(".confirm-dialog-cancel");
    if (cancel === null) throw new Error("confirm cancel action missing");
    expect(document.activeElement).toBe(cancel);
    await act(async () => {
      cancel.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(modalCount()).toBe(0);
    await hoverBand(container);
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("a modal opening under an open band closes the band", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <NewProjectDialog open={false} onClose={() => undefined} onCreate={() => undefined} />
      </ShellWith>,
    );

    await hoverBand(container);
    expect(navIsOpen(container)).toBe(true);

    await act(async () => {
      useAppStore.setState({ modalOpenTokens: new Set(["one"]) });
    });
    expect(navIsOpen(container)).toBe(false);
    await act(async () => root.unmount());
  });

  it("the line says it is unavailable while a modal is up", async () => {
    const { container, root } = await mount(<ShellWithProjectDialog />);
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver did not render");
    // Focusable (so the state stays perceivable) but named as unavailable.
    expect(sliver.getAttribute("aria-disabled")).toBe("true");

    // The modal's own Escape clears it, and the line is available again.
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(sliver.getAttribute("aria-disabled")).toBeNull();
    await act(async () => root.unmount());
  });
});
