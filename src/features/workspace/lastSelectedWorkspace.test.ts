// The last-selected workspace cell: written on change, never cleared on
// unmount, read by surfaces Workspace never mounts alongside.
import { describe, expect, it } from "vitest";
import { getLastSelectedWorkspaceKey, setLastSelectedWorkspaceKey } from "./lastSelectedWorkspace";
import { localWorkspaceKey } from "./hosts/hostIdentity";

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
});
