// @vitest-environment happy-dom

// What a tab says and draws about its kind: an agent names its provider and its
// title, a terminal or a file tab names its kind, only the agent reads in ink,
// and a child the Design profile birthed wears the Design pencil.

import { act } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Session, SessionStateSnapshot } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { PanelIcon, type PanelIconName } from "../panel/PanelIcon";
import { createWorkspaceSessionController } from "../workspaceSessions";
import { StripChip, ToolStripChip } from "./StripChip";
import { chipDisplay } from "./stripDisplay";
import { makeToolTab } from "./toolTabs";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = localWorkspaceKey("alpha") as WorkspaceKey;
const handlers = {
  onTabClick: () => undefined,
  onTabAuxClick: () => undefined,
  onRowContextMenu: () => undefined,
  onChipKeyDown: () => undefined,
  onClose: () => undefined,
};

const session = (kind: Session["kind"], title: string): Session => ({
  id: `s-${kind}`,
  workspaceId: "alpha",
  kind,
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
});

const tabButton = (): HTMLElement =>
  container.querySelector<HTMLElement>(".workspace-session-tab")!;

async function renderSession(
  subject: Session,
  selected = false,
  tooltip = "",
): Promise<HTMLElement> {
  await act(async () =>
    root.render(
      <StripChip
        session={subject}
        selected={selected}
        multiselected={false}
        tabIndex={0}
        display={chipDisplay(subject)}
        tooltip={tooltip}
        provenanceLines={[]}
        menuOpen={false}
        takeBack={false}
        onTakeBack={() => undefined}
        {...handlers}
      />,
    ),
  );
  return tabButton();
}

describe("a tab's tone and name", () => {
  it.each([
    ["claude", "Claude"],
    ["codex", "Codex"],
    ["pi", "Pi"],
    ["acp", "Agent"],
  ] as const)(
    "names a %s agent by its provider and its title, and reads in ink",
    async (kind, word) => {
      const tab = await renderSession(session(kind, "Tighten handoff"));

      expect(tab.classList.contains("workspace-session-tab-agent")).toBe(true);
      expect(tab.classList.contains("workspace-session-tab-quiet")).toBe(false);
      expect(tab.querySelector(".workspace-tab-label")?.textContent).toBe("Tighten handoff");
      expect(tab.querySelector(".workspace-sr-only")?.textContent).toMatch(new RegExp(`^${word},`));
    },
  );

  it("tells two generic agents apart by the provider each reports", async () => {
    const gemini = await renderSession({ ...session("acp", "Plan"), provider: "gemini" });
    expect(gemini.querySelector(".workspace-sr-only")?.textContent).toMatch(/^gemini,/);

    const named = await renderSession({ ...session("acp", "Plan"), provider: "claude" });
    expect(named.querySelector(".workspace-sr-only")?.textContent).toMatch(/^Claude,/);
  });

  it("makes a terminal a quiet tab that announces itself as a terminal", async () => {
    const tab = await renderSession(session("terminal", "Terminal 2"));

    expect(tab.classList.contains("workspace-session-tab-quiet")).toBe(true);
    expect(tab.querySelector(".workspace-sr-only")?.textContent).toMatch(/^Terminal,/);
  });

  it.each(["file", "diff"] as const)(
    "makes a %s tab quiet and announces its kind",
    async (kind) => {
      await act(async () =>
        root.render(
          <ToolStripChip
            tool={makeToolTab(kind, WORKSPACE, "docs/handoff.md")}
            selected={false}
            multiselected={false}
            tabIndex={0}
            tooltip="docs/handoff.md"
            menuOpen={false}
            {...handlers}
          />,
        ),
      );
      const tab = tabButton();

      expect(tab.classList.contains("workspace-session-tab-quiet")).toBe(true);
      expect(tab.querySelector(".workspace-sr-only")?.textContent).toMatch(
        kind === "file" ? /^File/ : /^Diff/,
      );
    },
  );

  it("keeps the selected state on a quiet tab as on an agent", async () => {
    const tab = await renderSession(session("terminal", "Terminal 2"), true);

    expect(tab.getAttribute("aria-selected")).toBe("true");
    expect(tab.classList.contains("workspace-session-tab-selected")).toBe(true);
  });
});

/** The pencil path the panel's own glyph draws, read back from its markup — the
 * chip shows this path or it is not the same pencil. */
function panelIconPath(name: PanelIconName): string | null {
  const host = document.createElement("div");
  host.innerHTML = renderToStaticMarkup(<PanelIcon name={name} />);
  return host.querySelector("path")?.getAttribute("d") ?? null;
}

function markPath(tab: HTMLElement): string | null {
  return tab.querySelector(".strip-kind svg path")?.getAttribute("d") ?? null;
}

function freshSessionStateSnapshot(id: string, profileId?: string): SessionStateSnapshot {
  return {
    id,
    workspaceId: "alpha",
    kind: "claude",
    title: "Design child",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    ...(profileId === undefined ? {} : { profileId }),
  };
}

/** The controller behind the strip, fed by hand: a push lands through the watch
 * the app subscribes to, and `list` answers the next refresh. */
function rosterHarness(list: () => Session[] = () => []) {
  const watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
    listener: null,
  };
  const controller = createWorkspaceSessionController({
    list: async () => list(),
    create: async () => {
      throw new Error("unused");
    },
    watch: async (listener) => {
      watched.listener = listener;
      return () => {
        watched.listener = null;
      };
    },
  });
  const end = controller.watch();
  return {
    controller,
    push: (snapshots: SessionStateSnapshot[]) => watched.listener?.(snapshots),
    end,
  };
}

describe("the mark a Design-profile child draws", () => {
  it("draws the panel's pencil where the provider mark would be", async () => {
    const tab = await renderSession({
      ...session("claude", "Landing page"),
      profileId: "design-builtin",
    });

    expect(tab.querySelector(".strip-kind .workspace-panel-icon")).not.toBeNull();
    expect(markPath(tab)).toBe(panelIconPath("design"));
    expect(tab.querySelector("[data-mark]")).toBeNull();
  });

  it("keeps the pencil under whatever name the row is renamed to", async () => {
    const tab = await renderSession({
      ...session("claude", "Shell 7"),
      displayName: "Pricing page",
      profileId: "design-builtin",
    });

    expect(tab.querySelector(".workspace-tab-label")?.textContent).toBe("Pricing page");
    expect(markPath(tab)).toBe(panelIconPath("design"));
  });

  it("keeps the provider mark on a Design-titled child of another profile", async () => {
    const tab = await renderSession({
      ...session("claude", "Design"),
      profileId: "user-profile-2",
    });

    expect(tab.querySelector('[data-mark="burst"]')).not.toBeNull();
    expect(tab.querySelector(".workspace-panel-icon")).toBeNull();
    expect(tab.querySelector(".workspace-sr-only")?.textContent).toBe("Claude, Running");
  });

  it("leaves a session that reports no profile exactly as it was", async () => {
    const tab = await renderSession(session("terminal", "Terminal 2"));

    expect(tab.querySelector('[data-mark="terminal"]')).not.toBeNull();
    expect(tab.querySelector(".workspace-panel-icon")).toBeNull();
    expect(tab.querySelector(".workspace-sr-only")?.textContent).toBe("Terminal, Running");
  });

  it("keeps the provider heard, the tooltip carried and the dot tone", async () => {
    const tab = await renderSession(
      { ...session("claude", "Landing page"), profileId: "design-builtin" },
      false,
      "Running",
    );

    expect(tab.querySelector(".workspace-sr-only")?.textContent).toBe(
      "Claude, Design profile, Running",
    );
    expect(tab.getAttribute("title")).toBe("Running");
    expect(tab.querySelector(".workspace-status-dot")?.classList.contains("strip-dot-live")).toBe(
      true,
    );
  });
});

describe("a roster row's profile reaching the mark", () => {
  it("draws the pencil for a push-only Design child, and keeps it when a later push and list stay silent", async () => {
    let listed: Session[] = [];
    const roster = rosterHarness(() => listed);
    try {
      roster.push([freshSessionStateSnapshot("child-1", "design-builtin")]);
      let tab = await renderSession(roster.controller.getState().sessions[0]!);
      expect(markPath(tab)).toBe(panelIconPath("design"));

      roster.push([freshSessionStateSnapshot("child-1")]);
      tab = await renderSession(roster.controller.getState().sessions[0]!);
      expect(markPath(tab)).toBe(panelIconPath("design"));

      listed = [{ ...session("claude", "Design child"), id: "child-1" }];
      await roster.controller.refresh();
      tab = await renderSession(roster.controller.getState().sessions[0]!);
      expect(markPath(tab)).toBe(panelIconPath("design"));
    } finally {
      roster.end();
    }
  });
});
