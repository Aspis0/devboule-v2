// @vitest-environment happy-dom

// Roster pushes rebuild every Session object, so a fresh agent-row build
// differs by identity even when nothing the rail reads moved. The stable hook
// must hand back the same map then — a status-only event must not rebuild the
// tree prop — and rebuild only what actually changed otherwise.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { Session } from "../../../types/ipc";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { useStableAgentRows } from "./agentRowViews";
import type { AgentRowView } from "./agentRowViews";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

type Rows = ReadonlyMap<WorkspaceKey, readonly AgentRowView[]>;

const session = (over: Partial<Session> = {}): Session => ({
  id: "s-1",
  workspaceId: "w-1",
  kind: "claude",
  title: "Tighten handoff",
  state: { type: "live", generation: 1 },
  elapsedMs: 12 * 60_000,
  activity: "idle",
  ...over,
});

const seen: Rows[] = [];

function Probe({ sessions }: { sessions: readonly Session[] }) {
  const rows = useStableAgentRows(sessions);
  useEffect(() => {
    seen.push(rows);
  }, [rows]);
  return null;
}

describe("the rail's stable agent rows", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    seen.length = 0;
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(sessions: readonly Session[]): Promise<void> {
    await act(async () => {
      root.render(<Probe sessions={sessions} />);
    });
  }

  function secondWorkspace(): Session[] {
    return [
      session({ id: "a", workspaceId: "w-1" }),
      session({ id: "b", workspaceId: "w-2", title: "Draft notes" }),
    ];
  }

  it("reuses the map when a roster push rebuilds identical sessions", async () => {
    await render(secondWorkspace());
    // New objects, same fields: exactly what a roster push delivers.
    await render(secondWorkspace().map((row) => ({ ...row })));

    expect(seen).toHaveLength(1);
  });

  it("ignores fields the rail never reads", async () => {
    await render(secondWorkspace());
    await render(secondWorkspace().map((row) => ({ ...row, resumable: true, cwd: "C:\\other" })));

    expect(seen).toHaveLength(1);
  });

  it("rebuilds only the workspaces that changed", async () => {
    const first = secondWorkspace();
    await render(first);
    const before = seen[0];
    const firstKey = localWorkspaceKey("w-1")!;
    const secondKey = localWorkspaceKey("w-2")!;
    const otherBefore = before.get(secondKey);
    await render([{ ...first[0], title: "Retitled" }, { ...first[1] }]);

    expect(seen).toHaveLength(2);
    const after = seen[1];
    expect(after).not.toBe(before);
    expect(after.get(firstKey)?.[0]?.title).toBe("Retitled");
    // The untouched workspace keeps its list object.
    expect(after.get(secondKey)).toBe(otherBefore);
  });
});
