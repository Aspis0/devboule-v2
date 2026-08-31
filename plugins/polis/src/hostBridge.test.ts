import { describe, expect, it } from "vitest";
import { formatHandshakeReadout, formatWorkspaceRootReadout } from "./hostBridge";

describe("backend overlay readout", () => {
  it("prints the granted workspace root and handshake", () => {
    const value = {
      root: "C:/repo",
      status: "ok",
      handshake: {
        protocolVersion: 1,
        instanceId: "plugin-1",
        pid: 4242,
        capabilities: ["ping", "workspace.root"],
      },
    };
    expect(formatWorkspaceRootReadout(value)).toBe("Bridge reply: workspace.root ok — C:/repo");
    expect(formatHandshakeReadout(value)).toBe(
      "Backend: handshake ok · protocol 1 · pid 4242 · ping, workspace.root",
    );
  });

  it("says so when the host did not grant a root", () => {
    expect(formatWorkspaceRootReadout({ root: "", status: "ok" })).toContain(
      "did not grant a root",
    );
    expect(formatHandshakeReadout({ root: "C:/repo", status: "ok" })).toContain(
      "handshake missing",
    );
  });
});
