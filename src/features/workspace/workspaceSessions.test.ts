import { describe, expect, it, vi } from "vitest";
import type { Session, SessionStateSnapshot } from "../../types/ipc";
import {
  chatCapableProviders,
  createWorkspaceSessionController,
  requiresConsent,
  sessionCreateFromProvider,
  sessionStateLabel,
} from "./workspaceSessions";

const liveSession = (id: string, title = id): Session => ({
  id,
  workspaceId: null,
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

describe("workspace session controller", () => {
  it("maps stream-json to kind claude and acp to kind acp plus provider id", () => {
    expect(
      sessionCreateFromProvider({
        id: "claude",
        executable: "claude.exe",
        acpAvailable: false,
        authentication: "unknown",
        protocol: "stream-json",
      }),
    ).toEqual({ kind: "claude", provider: null });
    expect(
      sessionCreateFromProvider({
        id: "grok",
        executable: "grok.exe",
        acpAvailable: true,
        authentication: "unknown",
        protocol: "acp",
      }),
    ).toEqual({ kind: "acp", provider: "grok" });
    expect(sessionCreateFromProvider(undefined)).toEqual({ kind: "acp", provider: null });
    expect(
      chatCapableProviders([
        {
          id: "codex",
          executable: "codex.exe",
          acpAvailable: false,
          authentication: "unknown",
          protocol: null,
        },
        {
          id: "grok",
          executable: "grok.exe",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
        },
      ]).map((provider) => provider.id),
    ).toEqual(["grok"]);
  });

  it("offers npx wrappers and flags them with requiresConsent", () => {
    const providers = [
      {
        id: "codex-acp",
        executable: "npx",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
        origin: "npx-wrapper" as const,
      },
      {
        id: "grok",
        executable: "grok.exe",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
        origin: "user-binary" as const,
      },
      {
        id: "bare",
        executable: "bare.exe",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
      },
    ];
    const capable = chatCapableProviders(providers);
    expect(capable.map((p) => p.id)).toEqual(["codex-acp", "grok", "bare"]);
    expect(requiresConsent(providers[0])).toBe(true);
    expect(requiresConsent(providers[1])).toBe(false);
    expect(requiresConsent(providers[2])).toBe(false);
  });

  it("does not list a provider whose protocol cannot be launched", () => {
    expect(
      chatCapableProviders([
        {
          id: "future",
          executable: "future.exe",
          acpAvailable: false,
          authentication: "unknown",
          protocol: "future-proto",
        },
        {
          id: "grok",
          executable: "grok.exe",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
        },
      ]).map((provider) => provider.id),
    ).toEqual(["grok"]);
  });

  it("excludes only registry wrappers covered by a better native provider", () => {
    const providers = [
      {
        id: "claude",
        executable: "claude.exe",
        acpAvailable: false,
        authentication: "unknown" as const,
        protocol: "stream-json" as const,
        origin: "user-binary" as const,
      },
      {
        id: "claude-acp",
        executable: "claude-acp@1.0.0",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
        origin: "npx-wrapper" as const,
        pickable: false,
      },
      {
        id: "codex-acp",
        executable: "codex-acp@1.0.0",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
        origin: "npx-wrapper" as const,
      },
      {
        id: "pi-acp",
        executable: "pi-acp@1.0.0",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
        origin: "npx-wrapper" as const,
      },
    ];

    expect(chatCapableProviders(providers).map((provider) => provider.id)).toEqual([
      "claude",
      "codex-acp",
      "pi-acp",
    ]);
    expect(
      chatCapableProviders([{ ...providers[1], pickable: undefined }, providers[2]]).map(
        (provider) => provider.id,
      ),
    ).toEqual(["claude-acp", "codex-acp"]);
  });

  it("loads terminal and ACP sessions and selects the first real session", async () => {
    const list = vi.fn(async () => [
      liveSession("terminal-1", "shell one"),
      {
        ...liveSession("agent-1", "agent"),
        kind: "acp" as const,
      },
      liveSession("terminal-2", "shell two"),
    ]);
    const create = vi.fn(async () => liveSession("terminal-3", "shell three"));
    const controller = createWorkspaceSessionController({ list, create });

    await controller.refresh();

    expect(controller.getState()).toMatchObject({
      sessions: [
        liveSession("terminal-1", "shell one"),
        { ...liveSession("agent-1", "agent"), kind: "acp" },
        liveSession("terminal-2", "shell two"),
      ],
      selectedSessionId: "terminal-1",
      error: null,
    });
  });

  it("adds the created terminal and selects it without inventing session data", async () => {
    const created = liveSession("terminal-3", "shell three");
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1")]),
      create: vi.fn(async () => created),
    });
    await controller.refresh();

    await controller.create();

    expect(controller.getState().sessions).toEqual([liveSession("terminal-1"), created]);
    expect(controller.getState().selectedSessionId).toBe("terminal-3");
  });

  it("keeps the daemon's real failure message when create is rejected", async () => {
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1")]),
      create: vi.fn(async () => {
        throw new Error("Authentication required: test-reason");
      }),
    });
    await controller.refresh();

    await controller.create();

    const error = controller.getState().error;
    expect(error).toContain("test-reason");
    expect(error).not.toContain("unreachable");
  });

  it("reads the daemon's wire message when create rejects with a non-Error object", async () => {
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1")]),
      create: vi.fn(() =>
        Promise.reject({ code: "io", message: "ACP request failed (-32000): test-reason" }),
      ),
    });
    await controller.refresh();

    await controller.create();

    const error = controller.getState().error;
    expect(error).toContain("test-reason");
    expect(error).not.toContain("[object Object]");
  });

  it("falls back to a generic message when create rejects with an empty error", async () => {
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1")]),
      create: vi.fn(async () => {
        throw new Error("   ");
      }),
    });
    await controller.refresh();

    await controller.create();

    expect(controller.getState().error).toBe("Could not create the agent session.");
  });

  it("clears the create error when it is dismissed", async () => {
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1")]),
      create: vi.fn(async () => {
        throw new Error("Authentication required: test-reason");
      }),
    });
    await controller.refresh();
    await controller.create();
    expect(controller.getState().error).toContain("test-reason");

    controller.dismissError();

    expect(controller.getState().error).toBeNull();
  });

  it("keeps a visible error when the daemon cannot list sessions", async () => {
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => {
        throw new Error("daemon unreachable");
      }),
      create: vi.fn(async () => liveSession("terminal-1")),
    });

    await controller.refresh();

    expect(controller.getState()).toMatchObject({
      sessions: [],
      selectedSessionId: null,
      error: "Could not load sessions. The daemon is unreachable.",
    });
  });

  it("shows observed silence with its elapsed age instead of calling it idle", () => {
    expect(sessionStateLabel({ type: "silent", generation: 1 }, 40 * 60 * 1000)).toBe(
      "silent · 40 minutes",
    );
    expect(sessionStateLabel({ type: "silent", generation: 1 })).toBe("silent · duration unknown");
  });

  it("labels a finished session with an uncertified transcript", () => {
    const state = {
      type: "ended" as const,
      generation: 1,
      code: 1,
      integrity: {
        kind: "truncated" as const,
        droppedFrames: 2,
        droppedBytes: 12 * 1024,
        trimmedBytes: 0,
      },
    };
    expect(sessionStateLabel(state)).toBe("ended · truncated");
  });

  it("updates the tab roster from a pushed session snapshot", async () => {
    const watched: {
      listener: ((snapshots: SessionStateSnapshot[]) => void) | null;
    } = { listener: null };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("terminal-1", "old title")]),
      create: vi.fn(async () => liveSession("terminal-2")),
      watch: vi.fn(async (listener) => {
        watched.listener = listener;
        return () => {
          watched.listener = null;
        };
      }),
    });

    const release = controller.watch();
    await controller.refresh();
    watched.listener?.([
      {
        id: "terminal-1",
        title: "killed shell",
        state: { type: "ended", generation: 1, code: 137, integrity: { kind: "complete" } },
        elapsedMs: 42,
      },
    ]);

    expect(controller.getState().sessions).toEqual([
      {
        ...liveSession("terminal-1", "old title"),
        title: "killed shell",
        state: { type: "ended", generation: 1, code: 137, integrity: { kind: "complete" } },
        elapsedMs: 42,
      },
    ]);
    release();
  });
});
