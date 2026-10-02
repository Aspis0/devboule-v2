// @vitest-environment happy-dom

// Every dialog the source scan finds, walked: each raises the shared modal-open
// signal, keeps its own focus and Escape, and releases on close.

import { act, useState } from "react";
import { NewProjectDialog } from "../components/NewProjectDialog";
import { getFocusableElements } from "../lib/focusableElements";
import { ProfileDialog } from "../features/settings/profiles/ProfileDialog";
import { buildSkillBlock } from "../features/design/skillLoader";
import type { BuiltInSkillIndexEntry } from "../features/design/builtInSkills";
import type { DesignSkillSelection } from "../features/design/designSettings";
import {
  DesignCraftSheet,
  DesignSkillModeControl,
  DesignToolbar,
} from "../features/design/DesignSurface";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { ContextPopover } from "../features/workspace/ContextPopover";
import { SessionRenameDialog } from "../features/workspace/strip/SessionRenameDialog";
import {
  ShellWith,
  cleanupModalsDom,
  hoverBand,
  modalCount,
  mount,
  navIsOpen,
  nullRef,
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
describe("walking every dialog the source finds", () => {
  /** The popover's owner, as ContextMeter is in production: the close drives the open state. */
  function ShellWithContextPopover({ onClose }: { onClose: () => void }) {
    const [open, setOpen] = useState(true);
    return (
      <ShellWith>
        <ContextPopover
          open={open}
          anchorRef={nullRef<HTMLButtonElement>()}
          onClose={() => {
            onClose();
            setOpen(false);
          }}
          numbers={{ used: 10, max: 100, percent: 10 }}
          live={false}
          plan={null}
          planRecordedAt={null}
          lastFinished={null}
        />
      </ShellWith>
    );
  }

  it("New project raises the signal, traps focus inside, and closes on Escape", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <NewProjectDialog open onClose={onClose} onCreate={() => undefined} />,
    );

    expect(modalCount()).toBe(1);
    const dialog = container.querySelector<HTMLElement>('[role="dialog"]');
    if (dialog === null) throw new Error("new project dialog missing");
    expect(dialog.contains(document.activeElement)).toBe(true);

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  // Escape from a composing field is the IME's cancel, not the dialog's
  // dismissal: the typed path must survive it.
  it("New project keeps its draft when Escape arrives from an open composition", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <NewProjectDialog open onClose={onClose} onCreate={() => undefined} />,
    );
    const field = container.querySelector<HTMLInputElement>("#workspace-project-input");
    if (field === null) throw new Error("project path input missing");

    await act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", { bubbles: true, key: "Escape", isComposing: true }),
      );
    });
    expect(onClose).not.toHaveBeenCalled();
    expect(container.querySelector('[role="dialog"]')).not.toBeNull();

    // Positive control: outside a composition the Escape still closes, so
    // a guard that returns unconditionally fails here.
    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  // The composition guard may swallow Escape, never the dialog's Tab trap:
  // focus containment is what keeps the modal a modal mid-composition.
  it("New project keeps its Tab trap during a composition", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <NewProjectDialog open onClose={onClose} onCreate={() => undefined} />,
    );
    const dialog = container.querySelector<HTMLElement>('[role="dialog"]');
    if (dialog === null) throw new Error("new project dialog missing");
    const focusable = getFocusableElements(dialog);
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (first === undefined || last === undefined || first === last) {
      throw new Error("dialog needs at least two focusables");
    }
    await act(async () => last.focus());

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
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  it("Profile raises the signal, lands focus in the card, and closes on Escape", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <ProfileDialog open title="Edit profile" busy={false} onClose={onClose}>
        {({ requestClose }) => (
          <form>
            <input aria-label="Profile name" />
            <button type="button" onClick={requestClose}>
              Cancel
            </button>
          </form>
        )}
      </ProfileDialog>,
    );

    expect(modalCount()).toBe(1);
    const card = container.querySelector<HTMLElement>(".edit-card");
    if (card === null) throw new Error("profile card missing");
    expect(card.contains(document.activeElement)).toBe(true);

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  // Escape from a composing field is the IME's cancel, not the card's
  // dismissal: the typed form must survive it.
  it("Profile keeps its form when Escape arrives from an open composition", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <ProfileDialog open title="Edit profile" busy={false} onClose={onClose}>
        {({ requestClose }) => (
          <form>
            <input aria-label="Profile name" />
            <button type="button" onClick={requestClose}>
              Cancel
            </button>
          </form>
        )}
      </ProfileDialog>,
    );
    const field = container.querySelector<HTMLInputElement>('input[aria-label="Profile name"]');
    if (field === null) throw new Error("profile name input missing");

    await act(async () => {
      field.dispatchEvent(
        new KeyboardEvent("keydown", { bubbles: true, key: "Escape", isComposing: true }),
      );
    });
    expect(onClose).not.toHaveBeenCalled();
    expect(container.querySelector(".edit-card")).not.toBeNull();

    // Positive control: outside a composition the Escape still closes.
    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  // Same contract as New project: the composition guard may swallow Escape,
  // never the card's Tab trap.
  it("Profile keeps its Tab trap during a composition", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <ProfileDialog open title="Edit profile" busy={false} onClose={onClose}>
        {({ requestClose }) => (
          <form>
            <input aria-label="Profile name" />
            <button type="button" onClick={requestClose}>
              Cancel
            </button>
          </form>
        )}
      </ProfileDialog>,
    );
    const card = container.querySelector<HTMLElement>(".edit-card");
    if (card === null) throw new Error("profile card missing");
    const focusable = getFocusableElements(card);
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (first === undefined || last === undefined || first === last) {
      throw new Error("profile card needs at least two focusables");
    }
    await act(async () => last.focus());

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
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  it("the Design skill picker raises the signal, focuses the chosen mode, and closes on Escape", async () => {
    const skillSelection: DesignSkillSelection = { version: 1, mode: "all", enabledSlugs: [] };
    const { container, root } = await mount(
      <DesignSkillModeControl
        skillSelection={skillSelection}
        onSkillModeChange={() => undefined}
        onCraftOpen={() => undefined}
        onCraftReadMore={() => undefined}
      />,
    );

    expect(modalCount()).toBe(0);
    const trigger = container.querySelector<HTMLButtonElement>(
      '[data-design-skill-mode-trigger="true"]',
    );
    if (trigger === null) throw new Error("craft mode trigger missing");
    await act(async () => trigger.click());

    expect(modalCount()).toBe(1);
    const picker = container.querySelector<HTMLElement>("#design-skill-picker");
    if (picker === null) throw new Error("skill picker missing");
    expect(picker.contains(document.activeElement)).toBe(true);

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(modalCount()).toBe(0);
    expect(document.activeElement).toBe(trigger);
    await act(async () => root.unmount());
  });

  it("the Design craft sheet raises the signal and closes on Escape", async () => {
    const onClose = vi.fn();
    const skillSelection: DesignSkillSelection = { version: 1, mode: "manual", enabledSlugs: [] };
    const { container, root } = await mount(
      <DesignCraftSheet
        open
        skillIndex={[] as readonly BuiltInSkillIndexEntry[]}
        skillSelection={skillSelection}
        selectedSkillSlugs={[]}
        resolvedSkillSlugs={null}
        appliedSkillSlugs={null}
        hasResolvedComposition={false}
        skillBlock={buildSkillBlock([], [])}
        resolvedSkillSlugSet={new Set()}
        automaticBaselineSlugSet={new Set()}
        droppedSkillSlugSet={new Set()}
        readOnly={false}
        onClose={onClose}
        onSkillToggle={() => undefined}
      />,
    );

    expect(modalCount()).toBe(1);
    if (container.querySelector(".design-craft-sheet") === null) {
      throw new Error("craft sheet missing");
    }

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  it("the Design history popover raises the signal, focuses itself, and closes on Escape", async () => {
    const { container, root } = await mount(
      <DesignToolbar
        folderControl={null}
        grounded
        outputMode="page"
        busy={false}
        onOutputModeChange={() => undefined}
        canSave={false}
        saved
        saving={false}
        saveError={null}
        canUndo={false}
        canRedo={false}
        historyRefreshKey={0}
        liveSessionId={null}
        onGroundingToggle={() => undefined}
        onSave={() => undefined}
        onUndo={() => undefined}
        onRedo={() => undefined}
        onHistoryOpen={() => true}
      />,
    );

    expect(modalCount()).toBe(0);
    const trigger = container.querySelector<HTMLButtonElement>(
      'button[aria-controls="design-history-popover"]',
    );
    if (trigger === null) throw new Error("history trigger missing");
    await act(async () => trigger.click());

    expect(modalCount()).toBe(1);
    const popover = container.querySelector<HTMLElement>("#design-history-popover");
    if (popover === null) throw new Error("history popover missing");
    expect(document.activeElement).toBe(popover);

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(modalCount()).toBe(0);
    expect(document.activeElement).toBe(trigger);
    await act(async () => root.unmount());
  });

  it("the close confirm raises the signal, focuses its primary action, and cancels on Escape", async () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    const { root } = await mount(
      <ConfirmDialog
        open
        title="Close tab"
        message="3 unsaved changes?"
        confirmLabel="Close tab"
        tone="danger"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    expect(modalCount()).toBe(1);
    // The ask portals to document.body, outside the container.
    const cancel = document.querySelector<HTMLButtonElement>(".confirm-dialog-cancel");
    if (cancel === null) throw new Error("confirm cancel action missing");
    // Cancel holds initial focus: Enter on a destructive ask must not destroy.
    expect(document.activeElement).toBe(cancel);

    // The ask's Escape lives on its card, so the key has to come from
    // the focused button and bubble up to it.
    await act(async () => {
      cancel.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  it("the rename dialog raises the signal, focuses its field, and cancels on Escape", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(
      <ShellWith>
        <SessionRenameDialog
          rename={{ sessionId: "s.4242.7", title: "worker one" }}
          onClose={onClose}
        />
      </ShellWith>,
    );

    expect(modalCount()).toBe(1);
    const field = container.querySelector<HTMLInputElement>('[role="dialog"] input');
    if (field === null) throw new Error("rename field missing");
    expect(document.activeElement).toBe(field);

    // The shell's protection, not just the token: the band's hover is the
    // outside press, and a modal holds the surface shut against it.
    await hoverBand(container);
    expect(navIsOpen(container)).toBe(false);

    await act(async () => {
      field.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });

  it("the context popover is a menu, not a modal: the band's open is its outside press", async () => {
    const onClose = vi.fn();
    const { container, root } = await mount(<ShellWithContextPopover onClose={onClose} />);

    // Informational, click-to-open, closed by any outside press: it belongs
    // to the menu registry, so the band dismisses it instead of going dead
    // over it.
    expect(modalCount()).toBe(0);
    // The popover portals to document.body, outside the container.
    expect(document.querySelector(".workspace-context-popover")).not.toBeNull();

    await hoverBand(container);
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(document.querySelector(".workspace-context-popover")).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });
});
