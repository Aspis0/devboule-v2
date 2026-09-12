import { describe, expect, it, vi } from "vitest";
import type {
  PeerRow,
  ProviderInfo,
  Session,
  SessionKind,
  SessionOriginKind,
  SessionStateSnapshot,
  Workspace,
} from "../../types/ipc";
import {
  chatCapableProviders,
  createWorkspaceSessionController,
  peerDeviceNames,
  requiresConsent,
  sessionCreateFromProvider,
  sessionOriginBadge,
  sessionOriginUnknown,
  sessionStateLabel,
} from "./workspaceSessions";
import { workspaceView } from "./workspaceProjects";

const liveSession = (id: string, title = id): Session => ({
  id,
  workspaceId: null,
  kind: "terminal",
  title,
  state: { type: "live", generation: 1 },
  elapsedMs: 0,
});

const pairedPhone: PeerRow = {
  deviceId: "device-phone",
  displayName: "Xiaomi 14",
  role: "client",
  publicKey: "cHVibGljLWtleQ==",
  keyFingerprint: "f9e8d7c6b5a4938271605f4e3d2c1b0a",
  bindingKind: "tailnet",
  bindingNodeName: "xiaomi-14.tail80a42d.ts.net.",
  bindingLoginName: "user@example.com",
  address: "100.74.116.126:47831",
  pairedAt: 1_760_000_000_000,
  revokedAt: null,
  caps: ["view", "send"],
  pairedByUser: null,
  online: true,
};

describe("workspace session controller", () => {
  it("maps stream-json, ACP, Pi RPC, and Codex app-server to their session kinds", () => {
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
    expect(
      sessionCreateFromProvider({
        id: "pi",
        executable: "pi",
        acpAvailable: false,
        authentication: "unknown",
        protocol: "pi-rpc",
      }),
    ).toEqual({ kind: "pi", provider: null });
    expect(
      sessionCreateFromProvider({
        id: "codex",
        executable: "codex.exe",
        acpAvailable: false,
        authentication: "unknown",
        protocol: "codex-app-server",
      }),
    ).toEqual({ kind: "codex", provider: null });
    expect(sessionCreateFromProvider(undefined)).toEqual({ kind: "acp", provider: null });
    expect(
      chatCapableProviders([
        {
          id: "codex",
          executable: "codex.exe",
          acpAvailable: false,
          authentication: "unknown",
          protocol: "codex-app-server",
        },
        {
          id: "grok",
          executable: "grok.exe",
          acpAvailable: true,
          authentication: "unknown",
          protocol: "acp",
        },
        {
          id: "pi",
          executable: "pi",
          acpAvailable: false,
          authentication: "unknown",
          protocol: "pi-rpc",
        },
      ]).map((provider) => provider.id),
    ).toEqual(["codex", "grok", "pi"]);
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
        pickable: false,
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
    expect(capable.map((p) => p.id)).toEqual(["grok", "bare"]);
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

  it("keeps a synthetic not-installed row out of the chat picker", () => {
    // Shape of the daemon's "known but not installed" rows: no protocol field
    // at all, npmPackage instead of a usable executable path. Defense here is
    // the frontend half; the daemon refuses these rows for chat separately.
    const synthetic: ProviderInfo = {
      id: "claude",
      executable: "",
      acpAvailable: false,
      authentication: "unknown",
      installed: false,
      npmPackage: "@anthropic-ai/claude-code",
      latestVersion: "2.0.0",
    };
    expect(chatCapableProviders([synthetic])).toEqual([]);
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
        // Native Claude uses stream-json, so this ACP wrapper stays in Settings.
        pickable: false,
      },
      {
        id: "codex-acp",
        executable: "codex-acp@1.0.0",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
        origin: "npx-wrapper" as const,
        // Native Codex uses app-server, so this ACP wrapper stays in Settings.
        pickable: false,
      },
      {
        id: "pi-acp",
        executable: "pi-acp@1.0.0",
        acpAvailable: true,
        authentication: "unknown" as const,
        protocol: "acp" as const,
        origin: "npx-wrapper" as const,
        // Native pi is the pickable pi-rpc provider; pi-acp lacks native-tool
        // permission requests, so its wrapper remains Settings-only.
        pickable: false,
      },
      {
        id: "pi",
        executable: "pi",
        acpAvailable: false,
        authentication: "unknown" as const,
        protocol: "pi-rpc" as const,
        origin: "user-binary" as const,
      },
    ];

    expect(chatCapableProviders(providers).map((provider) => provider.id)).toEqual([
      "claude",
      "pi",
    ]);
    expect(
      chatCapableProviders([
        { ...providers[1], pickable: undefined },
        providers[2],
        providers[3],
        providers[4],
      ]).map((provider) => provider.id),
    ).toEqual(["claude-acp", "pi"]);
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

  it("passes only the selected workspace id to session creation", async () => {
    const created = { ...liveSession("agent-2", "agent"), workspaceId: "workspace-42" };
    const create = vi.fn(async (_workspaceId: string | null, _kind?: SessionKind) => created);
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => []),
      create,
    });

    await controller.create("acp", null, "workspace-42");

    expect(create).toHaveBeenCalledWith("workspace-42", "acp", null);
    expect(controller.getState().sessions).toEqual([created]);
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
      list: vi.fn(async () => [
        { ...liveSession("terminal-1", "old title"), workspaceId: "workspace-1" },
      ]),
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
        workspaceId: "workspace-1",
        kind: "terminal",
        title: "killed shell",
        state: { type: "ended", generation: 1, code: 137, integrity: { kind: "complete" } },
        elapsedMs: 42,
      },
    ]);

    // The process is gone, so the tab leaves the strip; nothing stays selected
    // behind a tab that no longer renders.
    expect(controller.getState().sessions).toEqual([]);
    expect(controller.getState().selectedSessionId).toBeNull();
    release();
  });

  it("keeps a session the user opened explicitly even without a running process", async () => {
    const recovered = {
      ...liveSession("old-1", "restored agent"),
      kind: "acp" as const,
      state: {
        type: "recovered" as const,
        generation: 2,
        integrity: {
          kind: "unverifiable" as const,
          droppedFrames: 0,
          droppedBytes: 0,
          trimmedBytes: 0,
        },
      },
    };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("live-1")]),
      create: vi.fn(async () => liveSession("terminal-2")),
    });
    await controller.refresh();

    controller.open(recovered);

    expect(controller.getState().sessions.map((session) => session.id)).toEqual([
      "live-1",
      "old-1",
    ]);
    expect(controller.getState().selectedSessionId).toBe("old-1");
  });

  it("keeps a pushed attention state when an older list response resolves afterward", async () => {
    let resolveList!: (sessions: Session[]) => void;
    const list = vi.fn(
      () =>
        new Promise<Session[]>((resolve) => {
          resolveList = resolve;
        }),
    );
    const watched: {
      listener: ((snapshots: SessionStateSnapshot[]) => void) | null;
    } = { listener: null };
    const controller = createWorkspaceSessionController({
      list,
      create: vi.fn(async () => liveSession("terminal-2")),
      watch: vi.fn(async (listener) => {
        watched.listener = listener;
        return () => {
          watched.listener = null;
        };
      }),
    });

    const release = controller.watch();
    const refresh = controller.refresh();
    watched.listener?.([
      {
        id: "agent-1",
        workspaceId: "workspace-1",
        kind: "acp",
        title: "waiting agent",
        state: { type: "live", generation: 1 },
        elapsedMs: 10,
        attention: { reason: "permission", atMs: 42 },
      },
    ]);
    resolveList([liveSession("agent-1", "older list title")]);
    await refresh;

    expect(list).toHaveBeenCalledTimes(1);
    expect(controller.getState().sessions[0]).toMatchObject({
      id: "agent-1",
      attention: { reason: "permission", atMs: 42 },
    });
    release();
  });

  it("uses pushed identity for an unknown session and counts it in its workspace", async () => {
    const watched: {
      listener: ((snapshots: SessionStateSnapshot[]) => void) | null;
    } = { listener: null };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => []),
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
        id: "push-only",
        workspaceId: "workspace-agent",
        kind: "acp",
        title: "restored agent",
        state: { type: "live", generation: 1 },
        elapsedMs: 10,
      },
    ]);

    const sessions = controller.getState().sessions;
    expect(sessions[0]).toMatchObject({
      id: "push-only",
      workspaceId: "workspace-agent",
      kind: "acp",
    });
    const workspaceOne: Workspace = {
      id: "workspace-one",
      projectId: "project-1",
      title: "one",
      isolation: "local",
      path: "C:\\project-1",
    };
    const workspaceWithAgent: Workspace = {
      id: "workspace-agent",
      projectId: "project-1",
      title: "agent",
      isolation: "local",
      path: "C:\\project-1",
    };
    expect(workspaceView(workspaceOne, sessions).meta).toBe("0 live sessions · local");
    expect(workspaceView(workspaceWithAgent, sessions).meta).toBe("1 live session · local");
    release();
  });
});

describe("session origin badge", () => {
  it("names the device a peer session came from", () => {
    const names = peerDeviceNames([pairedPhone]);
    expect(
      sessionOriginBadge(
        { origin: { kind: "peer", deviceId: "device-phone", role: "client" } },
        names,
      ),
    ).toBe("from Xiaomi 14");
  });

  it("shows nothing for a local origin, exactly as before peer sessions", () => {
    const names = peerDeviceNames([pairedPhone]);
    expect(sessionOriginBadge({ origin: { kind: "local" } }, names)).toBeNull();
  });

  it("says the origin is unknown for a session the daemon sent none for", () => {
    // Absent is a third state, not a local one: only a daemon older than the
    // field sends it, and a silent tab would read exactly like a local session.
    const names = peerDeviceNames([pairedPhone]);
    expect(sessionOriginBadge({}, names)).toBe("origin unknown");
    expect(sessionOriginBadge({ origin: undefined }, new Map())).toBe("origin unknown");
    expect(sessionOriginUnknown({})).toBe(true);
    expect(sessionOriginUnknown({ origin: { kind: "local" } })).toBe(false);
  });

  it("says the origin is unknown for an origin kind it cannot read", () => {
    // The daemon's own `unknown`, for a journal row whose origin column would
    // not parse, and a kind no build has heard of: `local` is the one kind that
    // may go unmentioned, and neither of these is it. The mark on the pill comes
    // from the same predicate, so the tab cannot word a session one way and
    // colour it another.
    const names = peerDeviceNames([pairedPhone]);
    expect(sessionOriginBadge({ origin: { kind: "unknown" } }, names)).toBe("origin unknown");
    expect(sessionOriginBadge({ origin: { kind: "kiosk" as SessionOriginKind } }, new Map())).toBe(
      "origin unknown",
    );
    expect(sessionOriginUnknown({ origin: { kind: "unknown" } })).toBe(true);
    expect(sessionOriginUnknown({ origin: { kind: "kiosk" as SessionOriginKind } })).toBe(true);
    // A peer whose device is unnamed is still a named peer, not an unknown one.
    expect(sessionOriginUnknown({ origin: { kind: "peer" } })).toBe(false);
  });

  it("keeps the device id when no list has named the device yet", () => {
    expect(
      sessionOriginBadge({ origin: { kind: "peer", deviceId: "device-phone" } }, new Map()),
    ).toBe("from device-phone");
  });

  it("calls a peer origin that names no device unknown instead of dropping the badge", () => {
    // The card reads `Device: unknown` for this guard, so the tab says
    // `from unknown`: the session still came from a peer, and no badge at all
    // is the one reading it may not give.
    expect(sessionOriginBadge({ origin: { kind: "peer" } }, new Map())).toBe("from unknown");
    expect(
      sessionOriginBadge(
        { origin: { kind: "peer", role: "daemon" } },
        peerDeviceNames([pairedPhone]),
      ),
    ).toBe("from unknown");
  });

  it("keeps the name of a revoked device, whose sessions still exist", () => {
    const names = peerDeviceNames([{ ...pairedPhone, revokedAt: 1_760_000_100_000 }]);
    expect(names.get("device-phone")).toBe("Xiaomi 14");
  });

  it("keeps the origin a roster push carries for a session no list has described", async () => {
    const watched: {
      listener: ((snapshots: SessionStateSnapshot[]) => void) | null;
    } = { listener: null };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => []),
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
        id: "peer-push",
        workspaceId: null,
        kind: "acp",
        title: "remote agent",
        state: { type: "live", generation: 1 },
        elapsedMs: 5,
        origin: { kind: "peer", deviceId: "device-phone", role: "client" },
      },
    ]);

    expect(controller.getState().sessions[0]?.origin).toEqual({
      kind: "peer",
      deviceId: "device-phone",
      role: "client",
    });
    release();
  });

  it("does not let a push that omits the origin erase what the list said", async () => {
    const listed: Session = {
      ...liveSession("peer-1", "remote shell"),
      origin: { kind: "peer", deviceId: "device-phone", role: "client" },
    };
    const watched: {
      listener: ((snapshots: SessionStateSnapshot[]) => void) | null;
    } = { listener: null };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [listed]),
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
        id: "peer-1",
        workspaceId: null,
        kind: "terminal",
        title: "remote shell",
        state: { type: "live", generation: 1 },
        elapsedMs: 5,
      },
    ]);

    expect(controller.getState().sessions[0]?.origin).toEqual(listed.origin);
    release();
  });
});
