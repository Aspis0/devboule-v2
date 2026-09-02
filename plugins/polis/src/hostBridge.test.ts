// @vitest-environment happy-dom

import { describe, expect, it, vi } from "vitest";
import {
  cityHudLabel,
  formatCityFetchReadout,
  loadCity,
  pendingCityState,
  formatBackendFailureReadout,
  formatHandshakeReadout,
  formatWorkspaceRootReadout,
  hostResponseWithinLimit,
  isSessionFeed,
  sessionFeedToAgents,
  type SessionFeed,
  subscribeSessions,
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
      expect(formatBackendFailureReadout(new Error(message))).toContain(`Backend: ${state}`);
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

  it("rejects an oversized host response before the city reaches the iframe", () => {
    expect(hostResponseWithinLimit({ files: [{ path: "x".repeat(1024 * 1024) }] })).toBe(false);
  });

  it("names the host city degradation counters", () => {
    const state = {
      status: "host" as const,
      city: {
        files: Array.from({ length: 2 }, (_, index) => ({
          id: `src/${index}.ts`,
          path: `src/${index}.ts`,
          lines: 1,
          district: "src",
        })),
        imports: [],
        agents: [],
        findings: [],
        dataSource: "host" as const,
        truncatedFiles: 1,
        skippedFiles: 2,
      },
    };
    expect(formatCityFetchReadout(state)).toContain("at least 1 beyond the file cap, 2 skipped");
  });

  it("exposes a real pending state and falls back honestly when city.get fails", async () => {
    const fallback = {
      files: [],
      imports: [],
      agents: [],
      findings: [],
      dataSource: "fixture" as const,
    };
    expect(pendingCityState()).toEqual({ status: "pending", city: null });

    const invoke = vi.fn().mockResolvedValue({
      files: [{ id: "src/a.ts", path: "src/a.ts", lines: 1, district: "src" }],
      imports: [],
      agents: [{ fabricated: true }],
      findings: [{ fabricated: true }],
      dataSource: "fixture",
    });
    const host = await loadCity(invoke, fallback);
    expect(host.status).toBe("host");
    expect(host.city.dataSource).toBe("host");
    expect(host.city.agents).toEqual([{ fabricated: true }]);
    expect(host.city.findings).toEqual([]);
    expect(invoke).toHaveBeenCalledWith("city.get", undefined, expect.any(Number));
    expect(cityHudLabel(host.city)).toBe("Host city");
    expect(formatCityFetchReadout(host)).toBe("City: host · 1 files · 0 directed roads");

    const failedInvoke = vi.fn().mockRejectedValue(new Error("city root unreadable"));
    const fixture = await loadCity(failedInvoke, fallback);
    expect(fixture.status).toBe("fixture");
    if (fixture.status !== "fixture") throw new Error("expected fixture fallback");
    expect(fixture.city).toBe(fallback);
    expect(fixture.error).toBeInstanceOf(Error);
    expect(cityHudLabel(fixture.city)).toBe("Fixture city");
    expect(formatCityFetchReadout(fixture)).toContain(
      "City: fixture fallback — host city fetch refused — city root unreadable",
    );

    const malformed = await loadCity(
      vi.fn().mockResolvedValue({ files: "not-an-array" }),
      fallback,
    );
    expect(formatCityFetchReadout(malformed)).toContain("host city malformed");

    const timeout = Object.assign(new Error("request timed out"), { code: "timeout" });
    const timedOut = await loadCity(vi.fn().mockRejectedValue(timeout), fallback);
    expect(formatCityFetchReadout(timedOut)).toContain("host city timeout");
  });

  it("validates the privacy-safe session feed and turns every session into a roster agent", () => {
    const feed = {
      sessions: [
        { id: "one", provider: null, state: "working", title: "Unknown shell" },
        { id: "two", provider: "opencode", state: "finished", title: "OpenCode task" },
      ],
    } satisfies SessionFeed;
    expect(isSessionFeed(feed)).toBe(true);
    expect(sessionFeedToAgents(feed)).toEqual([
      { id: "one", provider: null, state: "working", fileId: null, title: "Unknown shell" },
      { id: "two", provider: "opencode", state: "finished", fileId: null, title: "OpenCode task" },
    ]);
    expect(isSessionFeed({ sessions: [{ ...feed.sessions[0], workspaceId: "secret" }] })).toBe(
      false,
    );
    expect(isSessionFeed({ ...feed, workspaceRoot: "secret" })).toBe(false);
  });

  it("subscribes to source-checked session events and removes its listener on close", async () => {
    const invoke = vi.fn().mockResolvedValue({
      sessions: [{ id: "one", provider: null, state: "working", title: "Unknown shell" }],
    });
    const updates: unknown[] = [];
    const subscription = await subscribeSessions(invoke, (agents) => updates.push(agents));

    expect(updates).toEqual([
      [{ id: "one", provider: null, state: "working", fileId: null, title: "Unknown shell" }],
    ]);
    const request = invoke.mock.calls[0];
    const payload = request?.[1] as { subscriptionId: string };
    window.dispatchEvent(
      new MessageEvent("message", {
        source: window.parent,
        data: {
          v: 1,
          id: payload.subscriptionId,
          kind: "event",
          event: "sessions.update",
          value: {
            sessions: [{ id: "two", provider: "codex", state: "working", title: "Codex" }],
          },
        },
      }),
    );
    expect(updates).toHaveLength(2);

    window.dispatchEvent(
      new MessageEvent("message", {
        source: window.parent,
        data: {
          v: 1,
          id: payload.subscriptionId,
          kind: "event",
          event: "sessions.update",
          value: {
            sessions: [
              { id: "large", provider: null, state: "working", title: "x".repeat(1024 * 1024) },
            ],
          },
        },
      }),
    );
    expect(updates).toHaveLength(2);

    window.dispatchEvent(
      new MessageEvent("message", {
        source: {} as Window,
        data: {
          v: 1,
          id: payload.subscriptionId,
          kind: "event",
          event: "sessions.update",
          value: { sessions: [{ id: "evil", provider: null, state: "working", title: "Evil" }] },
        },
      }),
    );
    expect(updates).toHaveLength(2);

    subscription.close();
    window.dispatchEvent(
      new MessageEvent("message", {
        source: window.parent,
        data: {
          v: 1,
          id: payload.subscriptionId,
          kind: "event",
          event: "sessions.update",
          value: { sessions: [{ id: "three", provider: null, state: "finished", title: "Three" }] },
        },
      }),
    );
    expect(updates).toHaveLength(2);
  });
});
