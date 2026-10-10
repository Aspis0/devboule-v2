// @vitest-environment happy-dom

// What the workspace row prints: the workspace's own name as its title (never
// the host's), a status dot in the slot the letter avatar held, and a meta line
// that carries the branch, the remote host and the compact diff — only when one
// of them has something to say. The row's menu and its title editor are pinned
// in WorkspaceTree.rename and WorkspaceArchive.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LOCAL_HOST_ID, localWorkspaceKey } from "../hosts/hostIdentity";
import type { WorkspaceView } from "../workspaceProjects";
import type { WorkspaceStat } from "./useWorkspaceStats";
import { WorkspaceRow } from "./WorkspaceRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const workspace = (over: Partial<WorkspaceView> = {}): WorkspaceView => ({
  id: "workspace-1",
  projectId: "project-1",
  hostId: LOCAL_HOST_ID,
  title: "devboule-v2",
  displayTitle: "devboule-v2",
  isolation: "local",
  path: "C:\\devboule-v2",
  agents: { working: 0, waiting: 0 },
  stateDot: null,
  ...over,
});

describe("the workspace row", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(
    view: WorkspaceView,
    facts: {
      stat?: WorkspaceStat;
      branch?: string;
      hostName?: string;
      remote?: boolean;
      hostOffline?: boolean;
    } = {},
  ): Promise<HTMLElement> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspaceRow
          workspace={view}
          workspaceKey={localWorkspaceKey(view.id)}
          hostName={facts.hostName ?? "This PC"}
          selected={false}
          stat={facts.stat}
          branch={facts.branch}
          onSelect={vi.fn()}
          onRename={vi.fn(async () => null)}
          onArchive={vi.fn(async () => null)}
          remote={facts.remote}
          hostOffline={facts.hostOffline}
        />,
      );
    });
    const row = container.querySelector<HTMLElement>(".workspace-row");
    if (row === null) throw new Error("the workspace row did not render");
    return row;
  }

  describe("title", () => {
    it("is the workspace's own name, never the host's, even when it matches the project", async () => {
      const row = await render(workspace({ title: "devboule-v2" }), {
        hostName: "This PC",
      });

      expect(row.querySelector(".workspace-row-title")?.textContent).toBe("devboule-v2");
      expect(row.textContent).not.toContain("This PC");
    });

    it("keeps a numbered name as it is printed", async () => {
      const row = await render(
        workspace({ title: "devboule-v2 2", displayTitle: "devboule-v2 2" }),
      );

      expect(row.querySelector(".workspace-row-title")?.textContent).toBe("devboule-v2 2");
    });
  });

  describe("status dot", () => {
    it("is the first child, in the slot the letter avatar used to hold", async () => {
      const row = await render(workspace());

      expect([...row.children].map((child) => child.className)).toEqual([
        "workspace-row-status workspace-row-status-idle",
        "workspace-row-body",
      ]);
      expect(row.querySelector(".sidebar-avatar")).toBeNull();
    });

    it("draws a running row as a breathing dot", async () => {
      const row = await render(workspace({ stateDot: "pulse" }));

      expect(row.querySelector(".workspace-row-status-running")).not.toBeNull();
      expect(row.querySelector(".workspace-row-status-running")?.classList).toContain("dot-pulse");
      expect(row.getAttribute("aria-label")).toContain("running");
    });

    it("draws a row waiting on the person as a solid attention dot, not breathing", async () => {
      const row = await render(
        workspace({ stateDot: "attention", agents: { working: 0, waiting: 1 } }),
      );

      expect(row.querySelector(".workspace-row-status-attention")).not.toBeNull();
      expect(row.querySelector(".workspace-row-status-attention")?.classList).not.toContain(
        "dot-pulse",
      );
      expect(row.getAttribute("aria-label")).toContain("needs you");
    });

    it("draws an idle row as a hollow ring: the slot is held, nothing is claimed", async () => {
      const row = await render(workspace({ stateDot: null }));

      expect(row.querySelector(".workspace-row-status-idle")).not.toBeNull();
      expect(row.getAttribute("aria-label")).toContain("idle");
    });

    it("never prints a word for the state: the dot and the accessible name carry it", async () => {
      const row = await render(
        workspace({ stateDot: "pulse", agents: { working: 2, waiting: 0 } }),
      );

      expect(row.textContent).not.toMatch(/working|waiting|ago|running unattended/);
    });
  });

  describe("meta line", () => {
    it("is absent for a clean local row with no branch: the title alone", async () => {
      const row = await render(workspace());

      expect(row.querySelector(".workspace-row-sub")).toBeNull();
      expect(row.classList.contains("workspace-row-two")).toBe(false);
    });

    it("prints the branch in mono on the meta line", async () => {
      const row = await render(workspace(), { branch: "main" });

      expect(row.querySelector(".workspace-row-sub .workspace-row-branch")?.textContent).toBe(
        "main",
      );
    });

    it("prints the remote host with its glyph on the meta line", async () => {
      const row = await render(workspace(), {
        remote: true,
        hostName: "MacBookPro.lan",
        branch: "feat/kalsa",
      });

      const host = row.querySelector(".workspace-row-sub .workspace-row-host");
      expect(host?.textContent).toContain("MacBookPro.lan");
      expect(host?.querySelector(".workspace-row-host-glyph")).not.toBeNull();
      expect(row.getAttribute("aria-label")).toContain("MacBookPro.lan");
    });

    it("prints no host on a local row, whatever the host is called", async () => {
      const row = await render(workspace(), { branch: "main", hostName: "This PC" });

      expect(row.querySelector(".workspace-row-host")).toBeNull();
      expect(row.textContent).not.toContain("This PC");
    });

    it("says offline once, on the meta line, for a remote host that does not answer", async () => {
      const row = await render(workspace(), {
        remote: true,
        hostOffline: true,
        hostName: "MacBookPro.lan",
      });

      expect(row.querySelector(".workspace-row-offline")?.textContent).toBe("offline");
    });
  });

  describe("compact diff", () => {
    it("prints the totals in short form, additions and deletions coloured apart", async () => {
      const row = await render(workspace(), {
        branch: "main",
        stat: { additions: 14_000, deletions: 1_100 } as WorkspaceStat,
      });

      expect(row.querySelector(".workspace-row-add")?.textContent).toBe("+14k");
      expect(row.querySelector(".workspace-row-del")?.textContent).toBe("−1.1k");
      expect(row.querySelector(".workspace-row-totals")?.parentElement?.className).toContain(
        "workspace-row-sub",
      );
      expect(row.textContent).not.toContain("+14000");
    });

    it("keeps the exact figures for a pointer's tooltip and a screen reader", async () => {
      const row = await render(workspace(), {
        branch: "main",
        stat: { additions: 10_655, deletions: 0 } as WorkspaceStat,
      });

      expect(row.getAttribute("title")).toContain("+10655 −0");
    });

    it("leaves the totals out of a clean tree", async () => {
      const row = await render(workspace(), {
        branch: "main",
        stat: { additions: 0, deletions: 0 } as WorkspaceStat,
      });

      expect(row.querySelector(".workspace-row-totals")).toBeNull();
    });
  });

  describe("accessible name", () => {
    it("reads the name, the status, then the branch and the remote host", async () => {
      const row = await render(workspace({ stateDot: "pulse" }), {
        branch: "main",
        remote: true,
        hostName: "MacBookPro.lan",
      });

      expect(row.getAttribute("aria-label")).toBe("devboule-v2, running, main, MacBookPro.lan");
    });
  });

  describe("row menu", () => {
    it("has a kebab on a local row, named for its workspace", async () => {
      const row = await render(workspace());
      const wrap = row.parentElement;
      const kebab = wrap?.querySelector<HTMLButtonElement>("button.workspace-row-kebab");

      expect(kebab?.getAttribute("aria-label")).toBe("Actions for devboule-v2");
      expect(kebab?.getAttribute("aria-haspopup")).toBe("menu");
    });

    it("has no kebab on a remote row: its menu would act on this machine", async () => {
      const row = await render(workspace(), { remote: true, hostName: "MacBookPro.lan" });

      expect(row.parentElement?.querySelector(".workspace-row-kebab")).toBeNull();
    });
  });
});
