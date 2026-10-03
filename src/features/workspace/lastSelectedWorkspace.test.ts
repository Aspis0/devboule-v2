// @vitest-environment happy-dom

// The last-selected workspace cell: written on change, never cleared on
// unmount, read by surfaces Workspace never mounts alongside — and, since the
// cell became a record, by the next run of the app.
import { describe, expect, it, vi } from "vitest";
import { getLastSelectedWorkspaceKey, setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { localWorkspaceKey } from "./hosts/hostIdentity";
import { LAST_WORKSPACE_STORAGE_KEY } from "./tabMemoryStorage";

/** A run: the module as the app loads it, reading what the last run left. */
function startApp(): Promise<typeof import("./lastSelectedWorkspace")> {
  vi.resetModules();
  return import("./lastSelectedWorkspace");
}

describe("lastSelectedWorkspace", () => {
  it("starts with no workspace and remembers the last selection", () => {
    setLastSelectedWorkspaceKey(null);
    expect(getLastSelectedWorkspaceKey()).toBeNull();
    setLastSelectedWorkspaceKey(localWorkspaceKey("w1")!);
    expect(getLastSelectedWorkspaceKey()).toBe("local:w1");
    setLastSelectedWorkspaceKey(localWorkspaceKey("w2")!);
    expect(getLastSelectedWorkspaceKey()).toBe("local:w2");
    setLastSelectedWorkspaceKey(null);
    expect(getLastSelectedWorkspaceKey()).toBeNull();
  });

  it("comes back to the workspace the last run was standing in", async () => {
    const first = await startApp();
    first.setLastSelectedWorkspaceKey(localWorkspaceKey("w1")!);

    const after = await startApp();
    expect(after.getLastSelectedWorkspaceKey()).toBe("local:w1");

    // Clearing the selection is what takes the workspace out of the record:
    // a run that starts with none must not land on the last one's row.
    after.setLastSelectedWorkspaceKey(null);
    const cleared = await startApp();
    expect(cleared.getLastSelectedWorkspaceKey()).toBeNull();
    expect(localStorage.getItem(LAST_WORKSPACE_STORAGE_KEY)).toBeNull();
  });

  it("starts where a record this build did not mint leaves off", async () => {
    localStorage.setItem(LAST_WORKSPACE_STORAGE_KEY, "not a key");

    const after = await startApp();

    expect(after.getLastSelectedWorkspaceKey()).toBeNull();
  });
});
