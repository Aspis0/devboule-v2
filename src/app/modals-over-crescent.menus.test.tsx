// @vitest-environment happy-dom

// Every menu the source scan finds, walked inside the real shell: the band's open
// is the outside press — the menu is gone and the nav is open.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { Shell } from "./Shell";
import { PickerChip } from "../components/PickerChip";
import { ProviderKebab } from "../features/settings/providers/ProviderKebab";
import { DeviceKebab } from "../features/settings/devices/DeviceKebab";
import { PaneHeaderKebab } from "../features/workspace/paneHeader/PaneHeaderKebab";
import { SidePanelTabs } from "../features/workspace/panel/SidePanelTabs";
import { DesignFolderControl } from "../features/design/DesignFolderControl";
import { FilesTreeView } from "../features/workspace/FilesTreeView";
import { ChangesTreeView } from "../features/workspace/ChangesTreeView";
import { SessionStrip } from "../features/workspace/strip/SessionStrip";
import { WorkspaceComposer } from "../features/workspace/WorkspaceComposer";
import {
  composerDrivers,
  composerProps,
  MENU_COMMANDS,
} from "../features/workspace/composerTestKit";
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
            onRename: null,
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
        anchorId: "s1",
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
          tabs={[]}
          activeTabId={null}
          selectTab={() => undefined}
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
          overviewSessions={[]}
          workspaceName={null}
          onOpenSession={() => undefined}
          selectedSessionId={null}
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
