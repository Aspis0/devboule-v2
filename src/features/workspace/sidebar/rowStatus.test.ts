import { describe, expect, it } from "vitest";
import { LOCAL_HOST_ID } from "../hosts/hostIdentity";
import type { WorkspaceView } from "../workspaceProjects";
import { rowStatus } from "./rowStatus";

const view = (stateDot: WorkspaceView["stateDot"]): WorkspaceView => ({
  id: "w-1",
  projectId: "p-1",
  hostId: LOCAL_HOST_ID,
  title: "devboule",
  displayTitle: "devboule",
  isolation: "local",
  path: "C:\\devboule",
  agents: { working: 0, waiting: 0 },
  stateDot,
});

describe("rowStatus", () => {
  it("maps a turn in progress to running", () => {
    expect(rowStatus(view("pulse"))).toEqual({ tone: "running", label: "running" });
  });

  it("maps an approval owed to needs you", () => {
    expect(rowStatus(view("attention"))).toEqual({ tone: "attention", label: "needs you" });
  });

  it("maps no activity to idle, never to a blank slot", () => {
    expect(rowStatus(view(null))).toEqual({ tone: "idle", label: "idle" });
  });
});
