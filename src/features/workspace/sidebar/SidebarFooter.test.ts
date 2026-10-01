import { describe, expect, it } from "vitest";
import type { DaemonStatus } from "../../../types/ipc";
import { daemonLabel } from "./SidebarFooter";

const RECONNECT =
  "Devboule is having trouble reaching its background service. Try reconnecting or restart Devboule.";

function status(over: Partial<DaemonStatus>): DaemonStatus {
  return {
    state: "connected",
    pid: 4321,
    instanceId: "i-1",
    protocolVersion: 18,
    clients: 1,
    capabilities: [],
    message: null,
    ...over,
  };
}

describe("daemonLabel", () => {
  it("keeps the supervisor's own diagnosis out of the tooltip", () => {
    const label = daemonLabel(
      status({
        state: "unresponsive",
        message:
          "The daemon has not answered status checks for at least 4 seconds (3 consecutive failures).",
      }),
    );
    expect(label).toBe(`daemon · ${RECONNECT}`);
    expect(label).not.toContain("status checks");
    expect(label).not.toContain("consecutive failures");
  });

  it("maps the status poll's unreachable token to the same advice", () => {
    expect(daemonLabel(status({ state: "disconnected", message: "daemon unreachable" }))).toBe(
      `daemon · ${RECONNECT}`,
    );
  });

  it("keeps the quiet labels the foot authors itself", () => {
    expect(daemonLabel(status({ state: "connecting", message: null }))).toBe("daemon · connecting");
    expect(daemonLabel(status({ state: "unresponsive", message: null }))).toBe(
      "daemon · not answering",
    );
    expect(daemonLabel(status({ state: "disconnected", message: null }))).toBe(
      "daemon · disconnected",
    );
    expect(daemonLabel(status({ state: "connected", message: null }))).toBe("daemon · pid 4321");
  });
});
