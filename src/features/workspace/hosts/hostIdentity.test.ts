import { describe, expect, it } from "vitest";
import {
  LOCAL_HOST_ID,
  isWorkspaceKey,
  localWorkspaceKey,
  parseWorkspaceKey,
  workspaceKey,
} from "./hostIdentity";

describe("hostIdentity", () => {
  it("composes the host and the workspace into one key", () => {
    expect(workspaceKey(LOCAL_HOST_ID, "w.1")).toBe("local:w.1");
    expect(localWorkspaceKey("w.1")).toBe("local:w.1");
  });

  it("splits at the first separator, so a workspace id may itself carry one", () => {
    expect(parseWorkspaceKey(localWorkspaceKey("repo:branch")!)).toEqual({
      hostId: LOCAL_HOST_ID,
      workspaceId: "repo:branch",
    });
  });

  it("refuses a blank half: no separator, an empty host, or an empty workspace", () => {
    expect(workspaceKey(LOCAL_HOST_ID, "")).toBeNull();
    expect(isWorkspaceKey("local:w.1")).toBe(true);
    expect(isWorkspaceKey("w.1")).toBe(false);
    expect(isWorkspaceKey("local:")).toBe(false);
    expect(isWorkspaceKey(":w.1")).toBe(false);
    expect(isWorkspaceKey(null)).toBe(false);
  });
});
