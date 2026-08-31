import { describe, expect, it } from "vitest";
import {
  formatBackendFailureReadout,
  formatHandshakeReadout,
  formatWorkspaceRootReadout,
} from "./hostBridge";

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

  it("names the measured backend failure state", () => {
    const cases: Array<[string, string]> = [
      ["plugin 'polis' did not declare a backend", "no backend declared"],
      ["backend verification failed: digest mismatch", "spawn failed"],
      ["plugin handshake refused by peer", "handshake refused"],
      ["plugin method was not in the granted capability set", "capability refused"],
      ["timed out: plugin invoke", "timeout"],
    ];
    for (const [message, state] of cases) {
      expect(formatBackendFailureReadout(new Error(message))).toContain(
        `Backend: ${state}`,
      );
    }
  });

  it("uses stable refusal codes instead of English error text", () => {
    expect(
      formatBackendFailureReadout({
        code: "workspace_unavailable",
        message: "workspace.root is unavailable because no project is open",
      }),
    ).toContain("Backend: no project open");
    expect(
      formatBackendFailureReadout({
        code: "workspace_confinement_refused",
        message: "workspace.root refused: path escaped",
      }),
    ).toContain("Backend: workspace root refused");
  });
});
