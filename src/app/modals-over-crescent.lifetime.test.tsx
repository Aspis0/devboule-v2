// @vitest-environment happy-dom

// The modal-open signal is tied to the component's lifetime: an unmount without a
// close, a StrictMode double effect or a double release can never leave the band shut.

import { act, StrictMode } from "react";
import { useModalOpen } from "../lib/modalOpen";
import { useAppStore } from "../store/appStore";
import {
  ShellWith,
  cleanupModalsDom,
  hoverBand,
  modalCount,
  mount,
  navIsOpen,
  resetModalsStore,
} from "./modals-over-crescent.harness";

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
describe("the modal-open signal is tied to the component's lifetime", () => {
  function Probe({ open }: { open: boolean }) {
    useModalOpen(open);
    return null;
  }

  it("an unmount without a close leaves the band working", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <Probe open />
      </ShellWith>,
    );
    expect(modalCount()).toBe(1);
    await hoverBand(container);
    expect(navIsOpen(container)).toBe(false);

    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);

    // The same shell, remounted: the leaked registration is gone.
    const second = await mount(
      <ShellWith>
        <div />
      </ShellWith>,
    );
    await hoverBand(second.container);
    expect(navIsOpen(second.container)).toBe(true);
    await act(async () => second.root.unmount());
  });

  it("StrictMode's double effect registers exactly once", async () => {
    const { root } = await mount(
      <StrictMode>
        <Probe open />
      </StrictMode>,
    );
    expect(modalCount()).toBe(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  it("two registrations at once each release their own", async () => {
    function TwoProbes() {
      return (
        <>
          <Probe open />
          <Probe open />
        </>
      );
    }
    const { root } = await mount(<TwoProbes />);
    expect(modalCount()).toBe(2);

    function OneProbe() {
      return <Probe open />;
    }
    await act(async () => {
      root.render(<OneProbe />);
    });
    expect(modalCount()).toBe(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  it("a double release cannot take the count below zero", async () => {
    const first = useAppStore.getState().openModal();
    const second = useAppStore.getState().openModal();
    expect(modalCount()).toBe(2);
    first();
    first();
    expect(modalCount()).toBe(1);
    second();
    second();
    expect(modalCount()).toBe(0);
  });
});
