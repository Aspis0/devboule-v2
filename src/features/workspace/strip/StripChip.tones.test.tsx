// @vitest-environment happy-dom

// What a tab says about its kind: an agent names its provider and its title, a
// terminal or a file tab names its kind, and only the agent reads in ink.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
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

async function renderSession(subject: Session, selected = false): Promise<HTMLElement> {
  await act(async () =>
    root.render(
      <StripChip
        session={subject}
        selected={selected}
        multiselected={false}
        tabIndex={0}
        display={chipDisplay(subject)}
        tooltip=""
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
