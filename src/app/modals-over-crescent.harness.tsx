// @vitest-environment happy-dom

// Shared harness: the mounting helpers, the shell wrappers and the store reset
// every modals-over-crescent test file uses.

import { act, useState } from "react";
import type { ReactNode, RefObject } from "react";
import { createRoot } from "react-dom/client";
import { vi } from "vitest";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../features/plugins/install", () => ({ chooseAndInstall: vi.fn() }));
vi.mock("../features/design/DesignHistoryList", () => ({ DesignHistoryList: () => null }));

import { NewProjectDialog } from "../components/NewProjectDialog";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { useAppStore } from "../store/appStore";
import { Shell } from "./Shell";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

export function modalCount(): number {
  return useAppStore.getState().modalOpenTokens.size;
}

export async function mount(node: ReactNode): Promise<{
  container: HTMLDivElement;
  root: ReturnType<typeof createRoot>;
}> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(node);
  });
  return { container, root };
}

/** The band's hover — the gesture that opens the nav. */
export async function hoverBand(container: HTMLElement): Promise<void> {
  const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
  if (sliver === null) throw new Error("crescent sliver did not render");
  await act(async () => {
    sliver.dispatchEvent(new Event("pointerover", { bubbles: true }));
  });
}

export function navIsOpen(container: HTMLElement): boolean {
  const navigation = container.querySelector<HTMLElement>(".crescent-nav");
  if (navigation === null) throw new Error("crescent nav did not render");
  return navigation.classList.contains("crescent-nav-open");
}

/** A dialog the shell holds shut: the defect's own configuration. */
export function ShellWith({ children }: { children: ReactNode }) {
  return <Shell activeSurface="workspace">{children}</Shell>;
}

/** A modal whose open state lives with the harness, so Escape can close it. */
export function ShellWithProjectDialog() {
  const [open, setOpen] = useState(true);
  return (
    <ShellWith>
      <NewProjectDialog open={open} onClose={() => setOpen(false)} onCreate={() => undefined} />
    </ShellWith>
  );
}

/** A ref no element is attached to yet — the popovers place from it or not. */
export function nullRef<T extends HTMLElement>(): RefObject<T | null> {
  return { current: null };
}

/** The destructive ask in the shell, with the state its parent would own. */
export function ShellWithConfirmDialog({ onCancel }: { onCancel: () => void }) {
  const [open, setOpen] = useState(true);
  return (
    <ShellWith>
      <ConfirmDialog
        open={open}
        title="Close tab"
        message="3 unsaved changes?"
        confirmLabel="Close tab"
        tone="danger"
        onConfirm={() => undefined}
        onCancel={() => {
          onCancel();
          setOpen(false);
        }}
      />
    </ShellWith>
  );
}

export function resetModalsStore(): void {
  useAppStore.setState({
    installError: null,
    plugins: null,
    installing: null,
    modalOpenTokens: new Set(),
    refreshPlugins: vi.fn(async () => undefined),
  });
}

export function cleanupModalsDom(): void {
  document.body.replaceChildren();
  useAppStore.setState({ modalOpenTokens: new Set() });
  vi.clearAllMocks();
}
