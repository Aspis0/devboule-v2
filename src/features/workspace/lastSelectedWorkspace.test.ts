// The last-selected workspace cell: written on change, never cleared on
// unmount, read by surfaces Workspace never mounts alongside.
import { describe, expect, it } from "vitest";
import { getLastSelectedWorkspaceId, setLastSelectedWorkspaceId } from "./lastSelectedWorkspace";

describe("lastSelectedWorkspace", () => {
  it("starts with no workspace and remembers the last selection", () => {
    setLastSelectedWorkspaceId(null);
    expect(getLastSelectedWorkspaceId()).toBeNull();
    setLastSelectedWorkspaceId("w1");
    expect(getLastSelectedWorkspaceId()).toBe("w1");
    setLastSelectedWorkspaceId("w2");
    expect(getLastSelectedWorkspaceId()).toBe("w2");
    setLastSelectedWorkspaceId(null);
    expect(getLastSelectedWorkspaceId()).toBeNull();
  });
});
