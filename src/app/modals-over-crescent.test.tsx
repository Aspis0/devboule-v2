// @vitest-environment happy-dom

// The crescent-over-modal contract, walked over every modal in the
// inventory: each one raises the shared modal-open signal (so the shell
// keeps the nav shut and the surface still), keeps its own Escape and
// focus behaviour, and releases the signal on close. The shell half of the
// contract — hover, click and surface switching — is driven here by a real
// modal mounted inside the real shell, the defect's own configuration.

import { act, useState } from "react";
import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../features/plugins/install", () => ({ chooseAndInstall: vi.fn() }));
vi.mock("../features/design/DesignHistoryList", () => ({ DesignHistoryList: () => null }));

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
import { useAppStore } from "../store/appStore";
import { Shell } from "./Shell";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function modalCount(): number {
  return useAppStore.getState().modalOpenCount;
}

async function mount(node: ReactNode): Promise<{
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

/** The defect's own configuration: a modal open inside the real shell. */
function ShellWithProjectDialog() {
  const [open, setOpen] = useState(true);
  return (
    <Shell activeSurface="workspace">
      <NewProjectDialog open={open} onClose={() => setOpen(false)} onCreate={() => undefined} />
    </Shell>
  );
}

beforeEach(() => {
  useAppStore.setState({
    installError: null,
    plugins: null,
    installing: null,
    modalOpenCount: 0,
    refreshPlugins: vi.fn(async () => undefined),
  });
});

afterEach(() => {
  document.body.replaceChildren();
  useAppStore.setState({ modalOpenCount: 0 });
  vi.clearAllMocks();
});

describe("the shell with a modal open (real modal, real shell)", () => {
  it("hovering the band does not open the nav while a modal is open, and does again after it closes", async () => {
    const { container, root } = await mount(<ShellWithProjectDialog />);

    expect(modalCount()).toBe(1);
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");

    await act(async () => {
      sliver.dispatchEvent(new Event("pointerover", { bubbles: true }));
    });
    expect(navigation.classList).not.toContain("crescent-nav-open");

    // The modal's own Escape still works while the nav is being kept shut.
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(modalCount()).toBe(0);

    await act(async () => {
      sliver.dispatchEvent(new Event("pointerover", { bubbles: true }));
    });
    expect(navigation.classList).toContain("crescent-nav-open");
    await act(async () => root.unmount());
  });

  it("a click on the line does not open the nav while a modal is open", async () => {
    const { container, root } = await mount(<ShellWithProjectDialog />);

    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    const navigation = container.querySelector<HTMLElement>(".crescent-nav");
    if (sliver === null || navigation === null) throw new Error("crescent did not render");
    await act(async () => sliver.click());
    expect(navigation.classList).not.toContain("crescent-nav-open");
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
});

describe("the modal-open signal the shell reads", () => {
  it("blocks a surface switch while a modal is open and allows one at zero", async () => {
    useAppStore.setState({ activeSurface: "workspace", modalOpenCount: 1 });
    await act(async () => {
      useAppStore.getState().selectSurface("polis");
    });
    expect(useAppStore.getState().activeSurface).toBe("workspace");

    useAppStore.setState({ modalOpenCount: 0 });
    await act(async () => {
      useAppStore.getState().selectSurface("polis");
    });
    expect(useAppStore.getState().activeSurface).toBe("polis");
  });
});

describe("walking every modal in the inventory", () => {
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
      <ProfileDialog title="Edit profile" busy={false} onClose={onClose}>
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
});
