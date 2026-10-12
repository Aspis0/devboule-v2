// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type {
  WorkspaceAgentEditFile,
  WorkspaceAgentEdits,
  WorkspaceAgentFileDiff,
} from "../../../types/ipc";

vi.mock("../../../lib/tauri", () => ({
  workspaceAgentEdits: vi.fn(),
  workspaceAgentDiff: vi.fn(),
}));

import { workspaceAgentDiff, workspaceAgentEdits } from "../../../lib/tauri";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { AGENT_CHANGES_NOTE, AgentChanges } from "./AgentChanges";
import { localAgentChangeReads } from "./agentChangeReads";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const NOW = 10_000_000;
const KEY = localWorkspaceKey("workspace-1") as WorkspaceKey;

const file = (
  path: string,
  over: Partial<WorkspaceAgentEditFile> = {},
): WorkspaceAgentEditFile => ({
  path,
  additions: 1,
  deletions: 0,
  lastAgent: "cia",
  lastAtMs: NOW - 10 * 60_000,
  calls: [
    { sessionId: "s", toolCallId: "t1" },
    { sessionId: "s", toolCallId: "t2" },
  ],
  truncated: false,
  ...over,
});

const edits = (files: WorkspaceAgentEditFile[], over: Partial<WorkspaceAgentEdits> = {}) =>
  ({
    files,
    totals: { additions: 452, deletions: 48 },
    truncated: false,
    error: null,
    ...over,
  }) as WorkspaceAgentEdits;

const diffOf = (path: string, over: Partial<WorkspaceAgentFileDiff> = {}) =>
  ({
    path,
    lines: [
      { kind: "header", text: "Edit 1 of 2" },
      { kind: "remove", text: ".hero {" },
      { kind: "add", text: ".hero { margin: 0; }" },
    ],
    status: "ok",
    error: null,
    ...over,
  }) as WorkspaceAgentFileDiff;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.spyOn(Date, "now").mockReturnValue(NOW);
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  vi.mocked(workspaceAgentDiff).mockImplementation(async (_id, path) => diffOf(path));
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.clearAllMocks();
  vi.restoreAllMocks();
});

async function render(over: Partial<React.ComponentProps<typeof AgentChanges>> = {}) {
  const onRefresh = vi.fn();
  await act(async () => {
    root.render(
      <AgentChanges reads={localAgentChangeReads(KEY)} onRefresh={onRefresh} {...over} />,
    );
  });
  return { onRefresh };
}

const names = () =>
  [...container.querySelectorAll(".workspace-agent-changes-name")].map((node) => node.textContent);
const selectedName = () =>
  container.querySelector(".workspace-agent-changes-row.is-selected .workspace-agent-changes-name")
    ?.textContent;

describe("the Changes panel of a folder that is not a repository", () => {
  it("lists the agents' files in path order, the first one chosen with its diff", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(
      edits([
        file("kalsa-site/index.html", { additions: 41, deletions: 18 }),
        file("kalsa-site/assets/site.css", { additions: 12, deletions: 30 }),
      ]),
    );
    await render();
    await act(async () => {});

    expect(container.querySelector(".workspace-agent-changes-head")?.textContent).toContain(
      "2 files",
    );
    expect(names()).toEqual(["site.css", "index.html"]);
    expect(selectedName()).toBe("site.css");
    expect(container.querySelector(".workspace-changes-diff-name")?.textContent).toBe("site.css");
    expect(container.querySelector(".workspace-changes-diff-dir")?.textContent).toBe(
      "kalsa-site/assets",
    );
  });

  it("says who edited the chosen file, how long ago, and how many edits", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(edits([file("site.css")]));
    await render();
    await act(async () => {});

    expect(container.querySelector(".workspace-changes-diff-meta")?.textContent).toBe(
      "cia · 10 min ago · 2 edits",
    );
  });

  it("reads a truncated list and a cut call list with a plus", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(
      edits([file("site.css", { truncated: true })], { truncated: true }),
    );
    await render();
    await act(async () => {});

    expect(container.querySelector(".workspace-agent-changes-head")?.textContent).toContain(
      "1+ file",
    );
    expect(container.querySelector(".workspace-changes-diff-meta")?.textContent).toBe(
      "cia · 10 min ago · 2+ edits",
    );
  });

  it("chooses a file by click and reads the diff of that file", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(
      edits([file("a.css"), file("b.css", { additions: 3, deletions: 0 })]),
    );
    await render();
    await act(async () => {});
    const second = container.querySelectorAll<HTMLButtonElement>(
      ".workspace-agent-changes-select",
    )[1];
    await act(async () => second?.click());

    expect(selectedName()).toBe("b.css");
    expect(vi.mocked(workspaceAgentDiff)).toHaveBeenLastCalledWith("workspace-1", "b.css");
    expect(container.querySelector(".workspace-changes-diff-name")?.textContent).toBe("b.css");
  });

  it("selects the file a transcript row asked to show, and keeps the person's next choice", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(edits([file("a.css"), file("b.css")]));
    await render({ revealPath: { path: "b.css", nonce: 1 } });
    await act(async () => {});
    expect(selectedName()).toBe("b.css");

    const first = container.querySelectorAll<HTMLButtonElement>(
      ".workspace-agent-changes-select",
    )[0];
    await act(async () => first?.click());
    expect(selectedName()).toBe("a.css");
  });

  it("names the folder as not a git repository in its footer", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(edits([file("site.css")]));
    await render();
    await act(async () => {});

    expect(container.querySelector(".workspace-agent-changes-note")?.textContent).toBe(
      AGENT_CHANGES_NOTE,
    );
  });

  it("says no agent has edited yet, with no list and no diff", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(
      edits([], { totals: { additions: 0, deletions: 0 } }),
    );
    await render();
    await act(async () => {});

    expect(container.querySelector(".workspace-agent-changes-head")?.textContent).toContain(
      "No changes by agents yet",
    );
    expect(container.querySelector(".workspace-agent-changes-list")).toBeNull();
    expect(container.querySelector(".workspace-changes-diff")).toBeNull();
    expect(container.querySelector(".workspace-agent-changes-note")).not.toBeNull();
  });

  it("says a diff is only partly kept, and keeps the lines it has", async () => {
    vi.mocked(workspaceAgentEdits).mockResolvedValue(edits([file("site.css")]));
    vi.mocked(workspaceAgentDiff).mockResolvedValue(diffOf("site.css", { status: "truncated" }));
    await render();
    await act(async () => {});

    expect(container.querySelector(".workspace-changes-diff-note")?.textContent).toBe(
      "Part of this diff is not kept.",
    );
    expect(container.querySelectorAll(".workspace-changes-diff-row")).toHaveLength(2);
  });

  it("shows a refused read as an alert", async () => {
    vi.mocked(workspaceAgentEdits).mockRejectedValue(new Error("the daemon is gone"));
    await render();
    await act(async () => {});

    expect(container.querySelector('[role="alert"]')).not.toBeNull();
  });
});
