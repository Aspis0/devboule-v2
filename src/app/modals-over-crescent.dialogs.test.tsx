// @vitest-environment happy-dom

// Every dialog the source scan finds, walked: each raises the shared modal-open
// signal, keeps its own focus and Escape, and releases on close.

import { act, useState } from "react";
import { NewProjectDialog } from "../components/NewProjectDialog";
import { ProfileDialog } from "../features/settings/profiles/ProfileDialog";
import { buildSkillBlock } from "../features/design/skillLoader";
import type { BuiltInSkillIndexEntry } from "../features/design/builtInSkills";
import type { DesignSkillSelection } from "../features/design/designSettings";
import {
  DesignCraftSheet,
  DesignSkillModeControl,
  DesignToolbar,
} from "../features/design/DesignSurface";
import { CloseConfirm } from "../features/workspace/strip/CloseConfirm";
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
      <CloseConfirm
        open
        anchorRef={nullRef<HTMLButtonElement>()}
        title="Close tab"
        message="3 unsaved changes?"
        confirmLabel="Close tab"
        onConfirm={onConfirm}
        onCancel={onCancel}
      />,
    );

    expect(modalCount()).toBe(1);
    // The ask portals to document.body, outside the container.
    const primary = document.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (primary === null) throw new Error("confirm primary action missing");
    expect(document.activeElement).toBe(primary);

    // The ask's Escape lives on its portal root, so the key has to come from
    // the focused button and bubble up to it.
    await act(async () => {
      primary.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
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
