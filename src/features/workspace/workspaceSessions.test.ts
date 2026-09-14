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
  DELEGATION_UNKNOWN_BADGE_LABEL,
  UNATTENDED_BADGE_LABEL,
  UNATTENDED_UNKNOWN_BADGE_LABEL,
  chatCapableProviders,
  createWorkspaceSessionController,
  peerDeviceNames,
  requiresConsent,
  sessionCreateFromProvider,
  sessionCreatorBadge,
  sessionDelegationBadges,
  sessionDelegationTakeBack,
  sessionDisplayNames,
  sessionOriginBadge,
  sessionOriginUnknown,
  sessionStateLabel,
  sessionTitle,
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

describe("session title", () => {
  it("prefers the session's own display name", () => {
    expect(sessionTitle({ ...liveSession("s.4242.7", "shell one"), displayName: "worker" })).toBe(
      "worker",
    );
    // The tab's label is this one string, so the trimming that makes an empty
    // name absent has to happen here rather than at every call site.
    expect(sessionTitle({ ...liveSession("s.4242.7"), displayName: "  worker  " })).toBe("worker");
    // A display name outranks a title, and the title still shows when the name
    // is only whitespace — the presence test is the trimmed one.
    expect(sessionTitle({ ...liveSession("s.4242.7", "shell one"), displayName: "   " })).toBe(
      "shell one",
    );
  });

  it("falls back exactly as before when the session has no display name", () => {
    expect(sessionTitle(liveSession("s.4242.7", "shell one"))).toBe("shell one");
    expect(sessionTitle(liveSession("s.4242.7", "  "))).toBe("Terminal s.4242.7");
    expect(sessionTitle({ ...liveSession("s.4242.7", ""), kind: "acp" })).toBe("Agent s.4242.7");
    // A recovered record comes back without a display name on older journals;
    // the id-prefix fallback is what covers that gap, so it stays intact.
    const recovered: Session = {
      ...liveSession("s.4242.7", ""),
      kind: "acp",
      state: {
        type: "recovered",
        generation: 1,
        integrity: { kind: "unverifiable", droppedFrames: 0, droppedBytes: 0, trimmedBytes: 0 },
      },
    };
    expect(sessionTitle(recovered)).toBe("Agent s.4242.7");
  });
});

describe("session identity badges", () => {
  const child = (createdBy?: string): Session => ({
    ...liveSession("s.4242.9", "worker"),
    ...(createdBy === undefined ? {} : { createdBy }),
  });

  it("names the creator a created session's roster row knows", () => {
    const names = sessionDisplayNames([
      { ...liveSession("s.4242.1", "design run"), displayName: "Design runner" },
      child("s.4242.1"),
    ]);
    expect(sessionCreatorBadge(child("s.4242.1"), names)).toBe("created by Design runner");
    // The map carries only named rows, so the badge's fallback below is reached
    // for a creator that has a title and no name.
    expect(names.has("s.4242.2")).toBe(false);
  });

  it("falls back to the creator's short id prefix when the roster has not named it", () => {
    expect(sessionCreatorBadge(child("s.4242.1"), new Map())).toBe("created by s.4242.1");
  });

  it("shows no creator badge on a session a person started", () => {
    // Absent `createdBy` is a human-started session, and also every row written
    // before the daemon kept the field.
    expect(sessionCreatorBadge(child(), sessionDisplayNames([liveSession("a")]))).toBeNull();
    expect(sessionCreatorBadge(child("   "), new Map())).toBeNull();
  });

  it("keeps a created session's display name and creator across a roster push", async () => {
    // The push carries neither field, so the controller's carried-over row is
    // the only thing standing between a live child and its own name and badge.
    const listed: Session = {
      ...liveSession("s.4242.9", "worker"),
      displayName: "worker",
      createdBy: "s.4242.1",
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
        id: "s.4242.9",
        workspaceId: null,
        kind: "terminal",
        title: "worker",
        state: { type: "live", generation: 1 },
        elapsedMs: 5,
      },
    ]);

    const pushed = controller.getState().sessions[0];
    expect(pushed?.displayName).toBe("worker");
    expect(pushed?.createdBy).toBe("s.4242.1");
    release();
  });
});

describe("session delegation badges", () => {
  it("shows nothing for a session that is not an agent-created child", () => {
    expect(sessionDelegationBadges(liveSession("human-1"))).toEqual([]);
  });

  it("shows nothing for an explicit no on both axes — pinned apart from the absent fixture above", () => {
    // The test above carries NEITHER field (the absent case); this one passes
    // the definite "no" values, so "silent because absent" and "silent
    // because no" are two pinned facts instead of one fixture's default.
    expect(
      sessionDelegationBadges({
        ...liveSession("human-1b"),
        delegation: { answered: 0, state: "off" },
        unattended: "no",
      }),
    ).toEqual([]);
  });

  it("shows nothing delegation-specific for a child nobody answers for", () => {
    expect(
      sessionDelegationBadges({
        ...liveSession("child-1"),
        delegation: { answered: 0, state: "off" },
        unattended: "no",
      }),
    ).toEqual([]);
  });

  it("names the creator's answering on an active child", () => {
    expect(
      sessionDelegationBadges({
        ...liveSession("child-2"),
        delegation: { answered: 0, state: "active" },
      }),
    ).toEqual([{ tone: "active", label: "answers to its creator" }]);
  });

  it("joins the answered count to the active pill once a card has been answered", () => {
    expect(
      sessionDelegationBadges({
        ...liveSession("child-3"),
        delegation: { answered: 3, state: "active" },
      }),
    ).toEqual([{ tone: "active", label: "answers to its creator · answered ×3" }]);
  });

  it("renders the loud unattended pill straight from the ledger", () => {
    expect(
      sessionDelegationBadges({
        ...liveSession("child-4"),
        delegation: { answered: 1, state: "unattended" },
        unattended: "yes",
      }),
    ).toEqual([{ tone: "unattended", label: UNATTENDED_BADGE_LABEL }]);
  });

  it("keeps the loud pill a birth fact: a row with unattended yes and no push yet still shows it", () => {
    // The list carries the tri-state before any push carries the ledger; the
    // pill must not wait for the push to tell the truth it already knows.
    expect(sessionDelegationBadges({ ...liveSession("child-5"), unattended: "yes" })).toEqual([
      { tone: "unattended", label: UNATTENDED_BADGE_LABEL },
    ]);
  });

  it("renders unknown as its own softer, present marker — never as nothing", () => {
    expect(
      sessionDelegationBadges({
        ...liveSession("child-6"),
        delegation: { answered: 0, state: "off" },
        unattended: "unknown",
      }),
    ).toEqual([{ tone: "unknown", label: UNATTENDED_UNKNOWN_BADGE_LABEL }]);
  });

  it("renders unknown additively beside the answering pill", () => {
    const badges = sessionDelegationBadges({
      ...liveSession("child-7"),
      delegation: { answered: 2, state: "active" },
      unattended: "unknown",
    });
    expect(badges).toEqual([
      { tone: "active", label: "answers to its creator · answered ×2" },
      { tone: "unknown", label: UNATTENDED_UNKNOWN_BADGE_LABEL },
    ]);
  });

  it("renders the ledger's own unknown marker for the app-minted unknown state", () => {
    expect(
      sessionDelegationBadges({
        ...liveSession("child-11"),
        delegation: { answered: 0, state: "unknown" },
      }),
    ).toEqual([{ tone: "unknown", label: DELEGATION_UNKNOWN_BADGE_LABEL }]);
  });

  it("renders a FIFTH wire state as the visible unknown badge — never as nothing", () => {
    // The daemon's vocabulary can grow before this build learns it; the cast
    // builds the value TypeScript cannot predict, and the badge table must
    // still give it a present, softer pill instead of a human-started row's
    // silence.
    const fifthState = "pending" as unknown as "off" | "active" | "unattended" | "unknown";
    expect(
      sessionDelegationBadges({
        ...liveSession("child-12"),
        delegation: { answered: 0, state: fifthState },
      }),
    ).toEqual([{ tone: "unknown", label: DELEGATION_UNKNOWN_BADGE_LABEL }]);
  });

  it("offers the take-back on the one row where flipping the setting ends an answering relationship", () => {
    expect(
      sessionDelegationTakeBack({
        ...liveSession("child-8"),
        delegation: { answered: 0, state: "active" },
      }),
    ).toBe(true);
  });

  it("offers the take-back on no unattended row: the click cannot take back a birth fact", () => {
    // The child was born able to run without asking and KEEPS that ability
    // after delegation is off, so the button's own promise ("stops every
    // agent from answering for its children") is empty here — and a control
    // that cannot act is noise beside the loud pill.
    expect(
      sessionDelegationTakeBack({
        ...liveSession("child-9"),
        delegation: { answered: 0, state: "unattended" },
      }),
    ).toBe(false);
  });

  it("offers the take-back nowhere else: not on humans, not on answered-off or unknown children", () => {
    expect(sessionDelegationTakeBack(liveSession("human-2"))).toBe(false);
    expect(
      sessionDelegationTakeBack({
        ...liveSession("child-10"),
        delegation: { answered: 4, state: "off" },
      }),
    ).toBe(false);
    expect(
      sessionDelegationTakeBack({
        ...liveSession("child-13"),
        delegation: { answered: 0, state: "unknown" },
      }),
    ).toBe(false);
    // A fifth wire value cannot earn the control either.
    const fifthState = "pending" as unknown as "off" | "active" | "unattended" | "unknown";
    expect(
      sessionDelegationTakeBack({
        ...liveSession("child-14"),
        delegation: { answered: 0, state: fifthState },
      }),
    ).toBe(false);
  });
});

describe("delegation facts ride the roster push", () => {
  const childSnapshot = (
    answered: number,
    state: "off" | "active" | "unattended",
  ): SessionStateSnapshot => ({
    id: "child-push",
    workspaceId: null,
    kind: "acp",
    title: "worker",
    state: { type: "live", generation: 1 },
    elapsedMs: 0,
    delegation: { answered, state },
    unattended: state === "unattended" ? "yes" : "no",
  });

  it("updates the answered count on every push instead of caching a stale one", async () => {
    const watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      listener: null,
    };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("child-push")]),
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

    watched.listener?.([childSnapshot(1, "active")]);
    const row = controller.getState().sessions[0];
    expect(sessionDelegationBadges(row)).toEqual([
      { tone: "active", label: "answers to its creator · answered ×1" },
    ]);

    watched.listener?.([childSnapshot(3, "active")]);
    const rowAfter = controller.getState().sessions[0];
    expect(sessionDelegationBadges(rowAfter)).toEqual([
      { tone: "active", label: "answers to its creator · answered ×3" },
    ]);
    release();
  });

  it("lets a push that omits the unattended marker keep the birth fact the row already knows", async () => {
    const watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      listener: null,
    };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("child-push")]),
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
    watched.listener?.([childSnapshot(0, "unattended")]);
    watched.listener?.([
      // The daemon carries the marker for every child; a push without it must
      // not un-see what earlier pushes established.
      {
        id: "child-push",
        workspaceId: null,
        kind: "acp",
        title: "worker",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
        delegation: { answered: 0, state: "unattended" },
      },
    ]);
    const row = controller.getState().sessions[0];
    expect(row.unattended).toBe("yes");
    expect(sessionDelegationBadges(row)).toEqual([
      { tone: "unattended", label: UNATTENDED_BADGE_LABEL },
    ]);
    release();
  });

  it("keeps the last described ledger when a push stops carrying it, and an explicit off clears it", async () => {
    // A push that SAYS "off" is the retraction, and it clears the pill. A
    // push that says NOTHING is not a retraction: the row it leaves
    // undescribed stays what earlier pushes said it was, because the ledger's
    // own absence reads "not a child" — and a child does not stop being one
    // between pushes. (Rewritten by the fix pass: the old test pinned the
    // erasure, which let an omitting push render a known child as a
    // human-started row — audit P2.12.)
    const watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      listener: null,
    };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("child-push")]),
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
    watched.listener?.([childSnapshot(0, "active")]);
    expect(controller.getState().sessions[0].delegation).toEqual({ answered: 0, state: "active" });
    watched.listener?.([
      {
        id: "child-push",
        workspaceId: null,
        kind: "acp",
        title: "worker",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
      },
    ]);
    expect(controller.getState().sessions[0].delegation).toEqual({ answered: 0, state: "active" });
    // An explicit off is the only thing that clears it.
    watched.listener?.([childSnapshot(0, "off")]);
    expect(controller.getState().sessions[0].delegation).toEqual({ answered: 0, state: "off" });
    release();
  });

  it("mints the unknown ledger for a child a push describes without ever describing its delegation", async () => {
    // A child known from `createdBy` with no ledger anywhere must not render
    // the benign absence of a human-started row: the honest third state is
    // "this is a child; no push has said who answers".
    const watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      listener: null,
    };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("child-mint")]),
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
        id: "child-mint",
        workspaceId: null,
        kind: "acp",
        title: "worker",
        state: { type: "live", generation: 1 },
        elapsedMs: 0,
        createdBy: "s.parent.1",
      },
    ]);
    const row = controller.getState().sessions[0];
    expect(row.createdBy).toBe("s.parent.1");
    expect(row.delegation).toEqual({ answered: 0, state: "unknown" });
    expect(sessionDelegationBadges(row)).toEqual([
      { tone: "unknown", label: DELEGATION_UNKNOWN_BADGE_LABEL },
    ]);
    release();
  });

  it("ratchets the unattended marker: a push claiming no over a known yes cannot downgrade it", async () => {
    // The marker is a fact of the session's birth and never downgraded — the
    // downgrade direction is the one that removes a warning.
    const watched: { listener: ((snapshots: SessionStateSnapshot[]) => void) | null } = {
      listener: null,
    };
    const controller = createWorkspaceSessionController({
      list: vi.fn(async () => [liveSession("child-ratchet")]),
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
    watched.listener?.([childSnapshot(0, "unattended")]);
    expect(controller.getState().sessions[0].unattended).toBe("yes");
    watched.listener?.([childSnapshot(0, "off")]); // carries unattended: "no"
    expect(controller.getState().sessions[0].unattended).toBe("yes");
    expect(sessionDelegationBadges(controller.getState().sessions[0])).toEqual([
      { tone: "unattended", label: UNATTENDED_BADGE_LABEL },
    ]);
    release();
  });
});
