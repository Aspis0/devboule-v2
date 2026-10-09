import { describe, expect, it } from "vitest";
import {
  getPendingRemoteCreate,
  remoteWorkspaceStoreKey,
  setPendingRemoteCreate,
  type PendingRemoteCreate,
} from "./remoteCreateKeys";

const KEY_A = remoteWorkspaceStoreKey("device-a", "workspace-one")!;
const KEY_B = remoteWorkspaceStoreKey("device-b", "workspace-one")!;

function pending(key: string): PendingRemoteCreate {
  return {
    key,
    deviceId: "device-a",
    workspaceId: "workspace-one",
    kind: "agent",
    provider: undefined,
    error: "The host stopped answering.",
  };
}

describe("the remote create retry store", () => {
  it("keeps one pending create per workspace", () => {
    setPendingRemoteCreate(KEY_A, pending("key-1"));
    expect(getPendingRemoteCreate(KEY_A)?.key).toBe("key-1");
    // Another host's workspace is another cell.
    expect(getPendingRemoteCreate(KEY_B)).toBeNull();
    setPendingRemoteCreate(KEY_A, null);
    expect(getPendingRemoteCreate(KEY_A)).toBeNull();
  });

  it("replaces the pending create when a new intent fails", () => {
    setPendingRemoteCreate(KEY_A, pending("key-1"));
    setPendingRemoteCreate(KEY_A, pending("key-2"));
    expect(getPendingRemoteCreate(KEY_A)?.key).toBe("key-2");
    setPendingRemoteCreate(KEY_A, null);
  });

  it("clearing an empty cell is a no-op", () => {
    setPendingRemoteCreate(KEY_B, null);
    expect(getPendingRemoteCreate(KEY_B)).toBeNull();
  });
});
