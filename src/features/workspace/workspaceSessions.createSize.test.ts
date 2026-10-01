// The workspace's "+ → Terminal" create runs before any view exists, so the
// only size it can send is the one another terminal already fitted.
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { recordFittedGrid } from "../terminal/lastFittedGrid";
import { createWorkspaceSessionController } from "./workspaceSessions";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
}));

const created = {
  id: "session-1",
  workspaceId: "workspace-1",
  kind: "terminal",
  title: "shell",
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
};

function createArgs(): unknown {
  const call = vi.mocked(invoke).mock.calls.find(([command]) => command === "session_create");
  return call?.[1];
}

describe("the workspace terminal create", () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(created as never);
  });

  // Runs first: the cache is module state and nothing has fitted yet.
  it("sends no size before any terminal has fitted", async () => {
    await createWorkspaceSessionController().create("terminal", null, "workspace-1");

    expect(createArgs()).toEqual({ workspaceId: "workspace-1", kind: "terminal", provider: null });
  });

  it("sends the grid the last terminal fitted", async () => {
    recordFittedGrid({ cols: 93, rows: 28 });

    await createWorkspaceSessionController().create("terminal", null, "workspace-1");

    expect(createArgs()).toEqual({
      workspaceId: "workspace-1",
      kind: "terminal",
      provider: null,
      cols: 93,
      rows: 28,
    });
  });

  it("leaves agents at the daemon default even after a fit", async () => {
    recordFittedGrid({ cols: 93, rows: 28 });

    await createWorkspaceSessionController().create("claude", null, "workspace-1");

    expect(createArgs()).toEqual({ workspaceId: "workspace-1", kind: "claude", provider: null });
  });
});
