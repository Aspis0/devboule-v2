// @vitest-environment happy-dom

// The crescent-over-modal contract, walked over every dialog and every menu
// the source scan finds: each dialog raises the shared modal-open signal (so
// the shell keeps the nav shut and the surface still), each menu closes when
// the band opens — the outside press, so the nav never opens over a picker
// and a nav click never unmounts one mid-action — and the signal itself is
// tied to the component's lifetime: an unmount without a close, a
// StrictMode double effect or a double release can never leave the band
// shut. The shell half of the contract is driven by real surfaces mounted
// inside the real shell, the defect's own configuration.

import { act, useState } from "react";
import type { ReactNode, RefObject } from "react";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../features/plugins/install", () => ({ chooseAndInstall: vi.fn() }));
vi.mock("../features/design/DesignHistoryList", () => ({ DesignHistoryList: () => null }));

import { NewProjectDialog } from "../components/NewProjectDialog";
import { PickerChip } from "../components/PickerChip";
import { ProfileDialog } from "../features/settings/profiles/ProfileDialog";
import { buildSkillBlock } from "../features/design/skillLoader";
import type { BuiltInSkillIndexEntry } from "../features/design/builtInSkills";
import type { DesignSkillSelection } from "../features/design/designSettings";
import {
  DesignCraftSheet,
  DesignSkillModeControl,
  DesignToolbar,
} from "../features/design/DesignSurface";
import { DesignFolderControl } from "../features/design/DesignFolderControl";
import { CloseConfirm } from "../features/workspace/strip/CloseConfirm";
import { SessionStrip } from "../features/workspace/strip/SessionStrip";
import { ContextPopover } from "../features/workspace/ContextPopover";
import { ProviderKebab } from "../features/settings/providers/ProviderKebab";
import { DeviceKebab } from "../features/settings/devices/DeviceKebab";
import { PaneHeaderKebab } from "../features/workspace/paneHeader/PaneHeaderKebab";
import { SidePanelTabs } from "../features/workspace/panel/SidePanelTabs";
import { FilesTreeView } from "../features/workspace/FilesTreeView";
import { ChangesTreeView } from "../features/workspace/ChangesTreeView";
import { WorkspaceComposer } from "../features/workspace/WorkspaceComposer";
import {
  composerDrivers,
  composerProps,
  MENU_COMMANDS,
} from "../features/workspace/composerTestKit";
import { useAppStore } from "../store/appStore";
import { useModalOpen } from "../lib/modalOpen";
import { Shell } from "./Shell";

(
  globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }
).IS_REACT_ACT_ENVIRONMENT = true;

function modalCount(): number {
  return useAppStore.getState().modalOpenTokens.size;
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

/** The band's hover — the gesture that opens the nav. */
async function hoverBand(container: HTMLElement): Promise<void> {
  const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
  if (sliver === null) throw new Error("crescent sliver did not render");
  await act(async () => {
    sliver.dispatchEvent(new Event("pointerover", { bubbles: true }));
  });
}

function navIsOpen(container: HTMLElement): boolean {
  const navigation = container.querySelector<HTMLElement>(".crescent-nav");
  if (navigation === null) throw new Error("crescent nav did not render");
  return navigation.classList.contains("crescent-nav-open");
}

/** A dialog the shell holds shut: the defect's own configuration. */
function ShellWith({ children }: { children: ReactNode }) {
  return <Shell activeSurface="workspace">{children}</Shell>;
}

/** A modal whose open state lives with the harness, so Escape can close it. */
function ShellWithProjectDialog() {
  const [open, setOpen] = useState(true);
  return (
    <ShellWith>
      <NewProjectDialog open={open} onClose={() => setOpen(false)} onCreate={() => undefined} />
    </ShellWith>
  );
}

/** A ref no element is attached to yet — the popovers place from it or not. */
function nullRef<T extends HTMLElement>(): RefObject<T | null> {
  return { current: null };
}

/** The destructive ask in the shell, with the state its parent would own. */
function ShellWithCloseConfirm({ onCancel }: { onCancel: () => void }) {
  const [open, setOpen] = useState(true);
  return (
    <ShellWith>
      <CloseConfirm
        open={open}
        anchorRef={nullRef<HTMLButtonElement>()}
        title="Close tab"
        message="3 unsaved changes?"
        confirmLabel="Close tab"
        onConfirm={() => undefined}
        onCancel={() => {
          onCancel();
          setOpen(false);
        }}
      />
    </ShellWith>
  );
}

beforeEach(() => {
  useAppStore.setState({
    installError: null,
    plugins: null,
    installing: null,
    modalOpenTokens: new Set(),
    refreshPlugins: vi.fn(async () => undefined),
  });
});

afterEach(() => {
  document.body.replaceChildren();
  useAppStore.setState({ modalOpenTokens: new Set() });
  vi.clearAllMocks();
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

  it("a portaled dialog holds the band shut too — the destructive close confirm", async () => {
    const onCancel = vi.fn();
    const { container, root } = await mount(<ShellWithCloseConfirm onCancel={onCancel} />);

    expect(modalCount()).toBe(1);
    await hoverBand(container);
    expect(navIsOpen(container)).toBe(false);

    // Its own Escape cancels the ask, and the band works again afterwards.
    const primary = document.querySelector<HTMLButtonElement>(".workspace-primary-action");
    if (primary === null) throw new Error("confirm primary action missing");
    await act(async () => {
      primary.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
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

describe("walking every dialog the source finds", () => {
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

  it("the context popover raises the signal and closes on Escape", async () => {
    const onClose = vi.fn();
    const { root } = await mount(
      <ContextPopover
        open
        anchorRef={nullRef<HTMLButtonElement>()}
        onClose={onClose}
        numbers={{ used: 10, max: 100, percent: 10 }}
        live={false}
        plan={null}
      />,
    );

    expect(modalCount()).toBe(1);
    // The popover portals to document.body, outside the container.
    const popover = document.querySelector<HTMLElement>(".workspace-context-popover");
    if (popover === null) throw new Error("context popover missing");

    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { bubbles: true, key: "Escape" }));
    });
    expect(onClose).toHaveBeenCalledTimes(1);
    await act(async () => root.unmount());
    expect(modalCount()).toBe(0);
  });
});

describe("walking every menu the source finds — the band's open is the outside press", () => {
  it("the mode picker closes when the band opens", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <PickerChip
          label="Mode"
          options={[{ id: "a", name: "A" }]}
          currentId="a"
          onSelect={() => undefined}
          chipTestId="mode-chip"
          optionTestId={(id) => `mode-option-${id}`}
        />
      </ShellWith>,
    );

    expect(modalCount()).toBe(0);
    const trigger = container.querySelector<HTMLButtonElement>(".workspace-mode-chip-trigger");
    if (trigger === null) throw new Error("mode chip trigger missing");
    await act(async () => trigger.click());
    expect(container.querySelector(".workspace-mode-menu")).not.toBeNull();

    await hoverBand(container);
    expect(container.querySelector(".workspace-mode-menu")).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the provider kebab menu closes when the band opens", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <ProviderKebab providerId="p" path="/bin/p" onRefresh={() => undefined} />
      </ShellWith>,
    );

    const kebab = container.querySelector<HTMLButtonElement>(".prov-kebab");
    if (kebab === null) throw new Error("provider kebab missing");
    await act(async () => kebab.click());
    expect(container.querySelector(".prov-menu")).not.toBeNull();

    await hoverBand(container);
    expect(container.querySelector(".prov-menu")).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the device kebab menu closes when the band opens", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <DeviceKebab displayName="Pixel" onRevoke={() => undefined} onLost={() => undefined} />
      </ShellWith>,
    );

    const kebab = container.querySelector<HTMLButtonElement>(".dev-kebab");
    if (kebab === null) throw new Error("device kebab missing");
    await act(async () => kebab.click());
    // The kebab's menu portals to document.body, outside the container.
    expect(document.querySelector(".dev-menu-pop")).not.toBeNull();

    await hoverBand(container);
    expect(document.querySelector(".dev-menu-pop")).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the pane header kebab menu closes when the band opens", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <PaneHeaderKebab
          menu={{
            copyPath: null,
            closeEntries: [{ key: "close", label: "Close", disabled: false, destructive: true }],
            onCloseEntry: () => undefined,
          }}
        />
      </ShellWith>,
    );

    const kebab = container.querySelector<HTMLButtonElement>(".pane-header-kebab");
    if (kebab === null) throw new Error("pane header kebab missing");
    await act(async () => kebab.click());
    // The kebab's menu portals to document.body, outside the container.
    expect(document.querySelector(".pane-header-menu")).not.toBeNull();

    await hoverBand(container);
    expect(document.querySelector(".pane-header-menu")).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the side panel kebab menu closes when the band opens", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <SidePanelTabs
          registry={[
            { id: "files", name: "Files", placement: "tab", icon: "files", render: () => null },
          ]}
          activeId="files"
          onSelect={() => undefined}
          onCollapse={() => undefined}
        />
      </ShellWith>,
    );

    const kebab = container.querySelector<HTMLButtonElement>(".workspace-panel-kebab button");
    if (kebab === null) throw new Error("panel kebab missing");
    await act(async () => kebab.click());
    expect(container.querySelector(".workspace-panel-menu")).not.toBeNull();

    await hoverBand(container);
    expect(container.querySelector(".workspace-panel-menu")).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the design folder picker closes when the band opens", async () => {
    const { container, root } = await mount(
      <ShellWith>
        <DesignFolderControl
          folders={[]}
          loading={false}
          refreshing={false}
          foldersError={null}
          selectionNotice={null}
          selectedWorkspaceId={null}
          selectionUnresolved={false}
          attachedPath={null}
          disabled={false}
          attachBusy={false}
          attachError={null}
          onOpen={() => undefined}
          onSelect={() => undefined}
          onAttach={async () => false}
          onUseFolder={async () => false}
        />
      </ShellWith>,
    );

    const trigger = container.querySelector<HTMLButtonElement>(
      '[data-design-folder-trigger="true"]',
    );
    if (trigger === null) throw new Error("folder trigger missing");
    await act(async () => trigger.click());
    expect(container.querySelector("#design-folder-picker")).not.toBeNull();

    await hoverBand(container);
    expect(container.querySelector("#design-folder-picker")).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the files tree row menu closes when the band opens", async () => {
    const onCloseMenu = vi.fn();
    const { container, root } = await mount(
      <ShellWith>
        <FilesTreeView
          cells={{
            "": {
              reply: {
                path: "",
                entries: [{ path: "/a.ts", name: "a.ts", kind: "file", size: 3 }],
                capped: false,
                skipped: 0,
                error: null,
              },
              failure: null,
            },
          }}
          expanded={new Set()}
          listId="files-tree"
          selection={null}
          onSelect={() => undefined}
          onToggle={() => undefined}
          menuPath="/a.ts"
          onToggleMenu={() => undefined}
          onCloseMenu={onCloseMenu}
          acting={false}
          renaming={null}
          onRenameChange={() => undefined}
          onCancelRename={() => undefined}
          onStartRename={() => undefined}
          onCommitRename={() => undefined}
          onDuplicate={() => undefined}
          onDelete={() => undefined}
          workspaceId="workspace-1"
        />
      </ShellWith>,
    );

    expect(container.querySelector(".workspace-tree-menu")).not.toBeNull();

    await hoverBand(container);
    expect(onCloseMenu).toHaveBeenCalledTimes(1);
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the changes tree row menu closes when the band opens", async () => {
    const onCloseMenu = vi.fn();
    const { container, root } = await mount(
      <ShellWith>
        <ChangesTreeView
          rows={[{ path: "/a.ts", status: "modified", additions: 1, deletions: 2, capped: false }]}
          inexact={false}
          selection={null}
          onSelect={() => undefined}
          onStage={() => undefined}
          onUnstage={() => undefined}
          onDiscard={() => undefined}
          menuPath="/a.ts"
          onToggleMenu={() => undefined}
          onCloseMenu={onCloseMenu}
          acting={false}
          workspaceId="workspace-1"
        />
      </ShellWith>,
    );

    expect(container.querySelector(".workspace-tree-menu")).not.toBeNull();

    await hoverBand(container);
    expect(onCloseMenu).toHaveBeenCalledTimes(1);
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the strip's new-tab and tab menus close when the band opens", async () => {
    const newTab = {
      open: true,
      creating: false,
      workspaceSelected: true,
      onToggle: vi.fn(),
      onAgent: vi.fn(),
      onTerminal: vi.fn(),
      onCloseMenu: vi.fn(),
    };
    const tabClose = {
      menu: {
        sessionId: "s1",
        entries: [{ key: "close" as const, label: "Close", disabled: false, destructive: true }],
      },
      anchorRef: nullRef<HTMLButtonElement>(),
      confirm: null,
      openMenu: vi.fn(),
      closeMenu: vi.fn(),
      closeSingle: vi.fn(),
      activateEntry: vi.fn(),
      activatePaneEntry: vi.fn(),
      confirmClose: vi.fn(),
      cancelClose: vi.fn(),
    };
    const { container, root } = await mount(
      <ShellWith>
        <SessionStrip
          sessions={[]}
          selectedSessionId={null}
          selectSession={() => undefined}
          tabSelection={{
            selection: new Set<string>(),
            announcement: "",
            handleTabClick: () => undefined,
            clearSelection: () => undefined,
          }}
          tabClose={tabClose}
          addButtonRef={nullRef<HTMLButtonElement>()}
          newTab={newTab}
          providerMenu={null}
          peerNames={new Map<string, string>()}
          resolveCreator={() => null}
          takeBackAvailable={false}
          onTakeBack={() => undefined}
          statusText="0 sessions"
        />
      </ShellWith>,
    );

    // Both menus portal to document.body, outside the container.
    expect(document.querySelectorAll('[role="menu"]').length).toBeGreaterThan(0);

    await hoverBand(container);
    expect(newTab.onCloseMenu).toHaveBeenCalledTimes(1);
    expect(tabClose.closeMenu).toHaveBeenCalledTimes(1);
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });

  it("the composer command menu closes when the band opens", async () => {
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <Shell activeSurface="workspace">
          <WorkspaceComposer
            {...composerProps({ onSend: () => undefined, onQueue: () => undefined })}
            availableCommands={MENU_COMMANDS}
          />
        </Shell>,
      );
    });
    const drive = composerDrivers(container);

    await drive.type("/");
    expect(drive.menu()).not.toBeNull();

    await hoverBand(container);
    expect(drive.menu()).toBeNull();
    expect(navIsOpen(container)).toBe(true);
    await act(async () => root.unmount());
  });
});

describe("a scrim may cover the band, but a band click arms nothing destructive", () => {
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
