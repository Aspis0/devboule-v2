// @vitest-environment happy-dom

// The workspace row of a remote host: its host rides the meta line with a
// server glyph, the row says offline in one word when the host does not answer,
// and no menu is offered for it. A local row never names its host, and its
// title is always the workspace's own name — the host never stands in for it.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LOCAL_HOST_ID, localWorkspaceKey, type HostId } from "../hosts/hostIdentity";
import type { WorkspaceView } from "../workspaceProjects";
import type { WorkspaceStat } from "./useWorkspaceStats";
import { WorkspaceRow } from "./WorkspaceRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const STAT: WorkspaceStat = { additions: 352, deletions: 66 };

function workspace(over: Partial<WorkspaceView> = {}): WorkspaceView {
  return {
    id: "workspace-1",
    projectId: "project-1",
    hostId: LOCAL_HOST_ID,
    title: "shell one",
    displayTitle: "shell one",
    isolation: "local",
    path: "C:\\code\\alpha",
    agents: { working: 0, waiting: 0 },
    stateDot: null,
    ...over,
  };
}

describe("the workspace row of a local workspace", () => {
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
    facts: { stat?: WorkspaceStat; branch?: string; hostName?: string } = {},
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
        />,
      );
    });
    const row = container.querySelector<HTMLElement>(".workspace-row");
    if (row === null) throw new Error("the workspace row did not render");
    return row;
  }

  it("prints no host line for this PC: the meta line holds the branch alone", async () => {
    const row = await render(workspace(), { branch: "main", hostName: "This PC" });

    expect(row.querySelector(".workspace-row-host")).toBeNull();
    expect(row.querySelector(".workspace-row-host-glyph")).toBeNull();
    expect(row.textContent).not.toContain("This PC");
  });

  it("prints the diff stats on the meta line's far end, the additions and deletions apart", async () => {
    const row = await render(workspace(), { branch: "main", stat: STAT });

    const totals = row.querySelector(".workspace-row-totals");
    expect(totals?.querySelector(".workspace-row-add")?.textContent).toBe("+352");
    expect(totals?.querySelector(".workspace-row-del")?.textContent).toBe("−66");
  });

  it("keeps the title as the workspace's own name even when it matches the project's", async () => {
    const row = await render(workspace({ title: "Alpha", displayTitle: "Alpha" }), {
      branch: "feature-x",
    });

    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("Alpha");
    expect(row.querySelector(".workspace-row-branch")?.textContent).toBe("feature-x");
  });

  it("never titles a row by its host, even when no branch is known", async () => {
    const row = await render(workspace({ title: "Alpha", displayTitle: "Alpha" }), {
      hostName: "Marcolenovo",
    });

    expect(row.querySelector(".workspace-row-title")?.textContent).toBe("Alpha");
    expect(row.textContent).not.toContain("Marcolenovo");
  });
});

describe("the row of a remote host", () => {
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

  async function render(facts: {
    remote?: boolean;
    hostOffline?: boolean;
    branch?: string;
    hostName?: string;
  }): Promise<HTMLElement> {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <WorkspaceRow
          workspace={workspace({ hostId: "device-one" as HostId, isolation: "local" })}
          workspaceKey={null}
          hostName={facts.hostName ?? "Marcolenovo"}
          selected={false}
          stat={undefined}
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

  it("prints its host on the meta line with a server glyph, beside the branch", async () => {
    const row = await render({ remote: true, hostOffline: false, branch: "main" });

    const host = row.querySelector(".workspace-row-sub .workspace-row-host");
    expect(host?.textContent).toContain("Marcolenovo");
    expect(host?.querySelector("svg.workspace-row-host-glyph")?.getAttribute("aria-hidden")).toBe(
      "true",
    );
    expect(row.querySelector(".workspace-row-branch")?.textContent).toBe("main");
  });

  it("marks an offline host with one short word, never a paragraph", async () => {
    const row = await render({ remote: true, hostOffline: true });

    expect(row.querySelector(".workspace-row-offline")?.textContent).toBe("offline");
    expect(row.textContent).not.toContain("unreachable");
    expect(row.textContent).not.toContain("stopped answering");
  });

  it("shows no offline word while the host answers", async () => {
    const row = await render({ remote: true, hostOffline: false });

    expect(row.querySelector(".workspace-row-offline")).toBeNull();
  });

  it("offers no kebab and no menu on a remote row", async () => {
    const row = await render({ remote: true, hostOffline: false });
    expect(row.parentElement?.querySelector(".workspace-row-kebab")).toBeNull();

    await act(async () => {
      row.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true }));
    });

    expect(container.querySelector(".workspace-row-menu")).toBeNull();
  });
});
