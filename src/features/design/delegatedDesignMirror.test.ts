// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";

const tauriMocks = vi.hoisted(() => ({
  attach: vi.fn(async () => 41),
  detach: vi.fn(async () => undefined),
  send: vi.fn(async () => {
    throw new Error("the mirror must never send");
  }),
  resume: vi.fn(async () => {
    throw new Error("the mirror must never resume");
  }),
  create: vi.fn(async () => {
    throw new Error("the mirror must never spawn");
  }),
  channels: [] as Array<(event: SessionEvent) => void>,
}));

// The display path's only daemon door is the history reopen's attach/detach.
// Every acting command throws here, so a mirror that reached for one would
// fail the test instead of silently passing.
vi.mock("../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../lib/tauri")>("../../lib/tauri");
  return {
    ...actual,
    sessionAttach: tauriMocks.attach,
    sessionDetach: tauriMocks.detach,
    sessionSend: tauriMocks.send,
    sessionResume: tauriMocks.resume,
    sessionCreate: tauriMocks.create,
    createSessionChannel: (onEvent: (event: SessionEvent) => void) => {
      tauriMocks.channels.push(onEvent);
      return {};
    },
  };
});

import { useAppStore } from "../../store/appStore";
import type { SessionEvent } from "../../types/ipc";
import type { DesignMessage } from "./designHost";
import type { DesignHost } from "./designHost";
import { openDesignHistoryEntry, type DesignHistoryOpenResult } from "./designHistoryOpen";
import {
  clearDelegatedMirrorPin,
  noteHumanOpenedHistory,
  resetDelegatedMirrorForTests,
  scheduleDelegatedDesignMirror,
  type DelegatedMirrorDeps,
} from "./delegatedDesignMirror";

type ChildFinishedEvent = Extract<SessionEvent, { type: "child_finished" }>;

interface CapturedOpen {
  sessionId: string;
  onResult: (result: DesignHistoryOpenResult) => void;
}

const TEST_DOCUMENT = {
  name: "",
  path: "",
  contextPrefix: "Editing",
  draftPlaceholder: "Describe the change…",
  noContextPlaceholder: "Describe what to generate…",
  initialState: { zoom: 1, saved: false, draft: "", hiddenLayerIds: [] },
  selectedLayerId: "",
  grounded: true,
  layers: [],
  messages: [],
  workingMessage: { title: "Generating…", desc: "Asking the agent." },
};

const HOST: DesignHost = {
  loadDocument: async () => ({ ...TEST_DOCUMENT }),
};

function childFinished(
  childSessionId: string,
  displayName = "design child",
  state: ChildFinishedEvent["state"] = "completed",
): ChildFinishedEvent {
  return {
    type: "child_finished",
    messageId: "m1",
    childSessionId,
    displayName,
    state,
    artifacts: [],
  };
}

function resetStore(): void {
  useAppStore.setState({
    designSession: {
      host: HOST,
      // Non-null, so writes take the append branch; the document-establishing
      // branch is covered by the host-creation test, which starts host-less.
      document: { ...TEST_DOCUMENT },
      messages: [],
      latestArtifact: null,
      generation: null,
      sectionNotes: [],
    },
  });
}

function fakeOpen(captured: CapturedOpen[]): DelegatedMirrorDeps["openHistory"] {
  return ((sessionId: string, deps: { onResult: (result: DesignHistoryOpenResult) => void }) => {
    captured.push({ sessionId, onResult: deps.onResult });
    return { dispose: () => undefined };
  }) as DelegatedMirrorDeps["openHistory"];
}

async function flush(): Promise<void> {
  for (let index = 0; index < 10; index += 1) await Promise.resolve();
}

function assistantCard(overrides: Partial<DesignMessage> & { id: string }): DesignMessage {
  return {
    role: "assistant",
    status: "done",
    title: "Older design",
    desc: "Reopened from design history.",
    sources: [],
    nodeIds: [],
    ...overrides,
  } as DesignMessage;
}

beforeEach(() => {
  resetDelegatedMirrorForTests();
  resetStore();
  tauriMocks.attach.mockClear();
  tauriMocks.detach.mockClear();
  tauriMocks.send.mockClear();
  tauriMocks.resume.mockClear();
  tauriMocks.create.mockClear();
  tauriMocks.channels.length = 0;
  vi.useRealTimers();
});

describe("delegated design mirror", () => {
  it("shows a finished child's artifact without any human opening", async () => {
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1", "Landing page"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();
    expect(captured.map((open) => open.sessionId)).toEqual(["child-1"]);

    await Promise.resolve();
    captured[0].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toMatchObject({ html: "<main>Delegated</main>" });
    expect(session.messages).toHaveLength(1);
    const message = session.messages[0];
    expect(message?.role).toBe("assistant");
    if (message?.role === "assistant") {
      expect(message.artifactHtml).toBe("<main>Delegated</main>");
      expect(message.title).toBe("Landing page");
    }
  });

  it("leaves a human-pinned older entry in place when a later child finishes", async () => {
    useAppStore.setState({
      designSession: {
        host: HOST,
        document: null,
        messages: [
          assistantCard({
            id: "old-card",
            title: "Older design",
            artifactHtml: "<main>Old</main>",
          }),
        ],
        latestArtifact: { html: "<main>Old</main>" },
        generation: null,
        sectionNotes: [],
      },
    });
    noteHumanOpenedHistory("old-child");

    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("new-child", "Newer design"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();

    // Pinned means not even read: no replay starts for the newer child.
    expect(captured).toHaveLength(0);
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toMatchObject({ html: "<main>Old</main>" });
    expect(session.messages).toHaveLength(1);

    // The pin is the human's to release: after it clears the same arrival mirrors.
    clearDelegatedMirrorPin();
    scheduleDelegatedDesignMirror(childFinished("new-child", "Newer design"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();
    expect(captured.map((open) => open.sessionId)).toEqual(["new-child"]);
  });

  it("writes nothing when the human is already viewing that exact child", async () => {
    noteHumanOpenedHistory("child-1");
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();

    expect(captured).toHaveLength(0);
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);
  });

  it("ignores a child whose state is not completed, even with design in its transcript", async () => {
    // A canceled child can carry a fenced block from its dead run; the panel
    // must not present it as finished work. The history entry still records
    // the finish, where the human reads the true state.
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1", "Canceled work", "canceled"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();

    // Not even replayed: the state alone disqualifies the arrival.
    expect(captured).toHaveLength(0);
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toBeNull();
    expect(session.messages).toHaveLength(0);
  });

  it("shows the design arrival when a later arrival has nothing to show", async () => {
    // The exact interleave the sequence guard must survive: A carries design
    // but replays slowly, B finishes later with no design at all. B must not
    // consume the slot — "latest wins" means the latest arrival that has
    // something to show.
    const captured: CapturedOpen[] = [];
    const deps = { openHistory: fakeOpen(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(childFinished("child-a", "Design work"), deps);
    scheduleDelegatedDesignMirror(childFinished("child-b", "Coder work"), deps);
    await flush();
    expect(captured.map((open) => open.sessionId)).toEqual(["child-a", "child-b"]);

    // The coder's replay resolves first, with nothing: the panel is untouched.
    captured[1].onResult({
      status: "failed",
      message: "The transcript could not be opened.",
    });
    await flush();
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);

    // The design arrival resolves after — and still lands.
    captured[0].onResult({ status: "artifact", html: "<main>A</main>" });
    await flush();
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toMatchObject({ html: "<main>A</main>" });
    expect(session.messages).toHaveLength(1);
  });

  it("lets the later arrival win when two children finish close together", async () => {
    const captured: CapturedOpen[] = [];
    const deps = { openHistory: fakeOpen(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(childFinished("child-a", "First"), deps);
    scheduleDelegatedDesignMirror(childFinished("child-b", "Second"), deps);
    await flush();
    expect(captured.map((open) => open.sessionId)).toEqual(["child-a", "child-b"]);

    captured[1].onResult({ status: "artifact", html: "<main>B</main>" });
    await flush();
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>B</main>",
    });

    // The earlier arrival's slow replay is stale and must not overwrite the newer one.
    captured[0].onResult({ status: "artifact", html: "<main>A</main>" });
    await flush();
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>B</main>",
    });
  });

  it("writes nothing when the replay finds no design work", async () => {
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();

    // The true no-design path: a coder child finishes, its replay holds no
    // fenced HTML, and the reopen reports failure — not a timeout.
    captured[0].onResult({
      status: "failed",
      message: "The transcript could not be opened.",
    });
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toBeNull();
    expect(session.messages).toHaveLength(0);
  });

  it("keeps the current panel when the replay times out: an unproven absence clears nothing", async () => {
    useAppStore.setState({
      designSession: {
        host: HOST,
        document: null,
        messages: [
          assistantCard({
            id: "old-card",
            title: "Older design",
            artifactHtml: "<main>Old</main>",
          }),
        ],
        latestArtifact: { html: "<main>Old</main>" },
        generation: null,
        sectionNotes: [],
      },
    });
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();

    captured[0].onResult({
      status: "timeout",
      message: "The transcript did not produce a design within 5 seconds.",
    });
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toMatchObject({ html: "<main>Old</main>" });
    expect(session.messages).toHaveLength(1);
  });

  it("writes nothing for an oversized artifact: unrenderable is not an error card", async () => {
    // The artifact pipeline caps what the panel can render; a child whose
    // design exceeds it still finished design work, but the panel cannot
    // show it — and a coder's missing design looks identical from here. The
    // mirror parses no message text: every failed replay resolves to nothing,
    // and the oversized run stays reachable through its history entry, where
    // the surface reports the too-large message itself.
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();

    captured[0].onResult({
      status: "failed",
      message: "Artifact too large to display (maximum 256 KiB).",
    });
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toBeNull();
    expect(session.messages).toHaveLength(0);
  });

  it("replays through attach and detach only: no resume, send or spawn", async () => {
    vi.useFakeTimers();
    try {
      scheduleDelegatedDesignMirror(childFinished("child-1"), {
        openHistory: openDesignHistoryEntry,
        store: useAppStore,
      });
      await Promise.resolve();
      expect(tauriMocks.attach).toHaveBeenCalledTimes(1);
      expect(tauriMocks.channels).toHaveLength(1);

      const emit = tauriMocks.channels[0];
      emit({
        type: "agent_message",
        messageId: "assistant-1",
        text: "```html\n<main>Delegated</main>\n```",
      });
      vi.advanceTimersByTime(400);
      await flush();

      expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
        html: "<main>Delegated</main>",
      });
      expect(tauriMocks.detach).toHaveBeenCalledTimes(1);
      expect(tauriMocks.send).not.toHaveBeenCalled();
      expect(tauriMocks.resume).not.toHaveBeenCalled();
      expect(tauriMocks.create).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("creates the Design host on first delegation so the panel has something to mirror", async () => {
    useAppStore.setState({
      designSession: {
        host: null,
        document: null,
        messages: [],
        latestArtifact: null,
        generation: null,
        sectionNotes: [],
      },
    });
    const freshHost: DesignHost = {
      // A production host loads pure defaults; the double returns the same
      // shape so the mirror can establish the document it owes the loader.
      loadDocument: async () => ({ ...TEST_DOCUMENT }),
    };
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1", "First delegation"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
      // The production default dynamic-imports the real host factory; the
      // import's own ticks would make this test wait on module loading
      // instead of on the mirror, so the factory is injected and the test
      // asserts what the mirror does with the host it is given.
      ensureHost: async () => freshHost,
    });
    await flush();
    captured[0].onResult({ status: "artifact", html: "<main>First</main>" });
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.host).toBe(freshHost);
    expect(session.latestArtifact).toMatchObject({ html: "<main>First</main>" });
  });

  it("adopts a session that appears while the host factory resolves", async () => {
    // The ensureHost window: host null at schedule time, the human opens
    // Design (host + loaded document) while the factory is still resolving.
    // Establishing the fresh host would zero that live session and strand
    // the mounted surface on a host the store no longer holds.
    useAppStore.setState({
      designSession: {
        host: null,
        document: null,
        messages: [],
        latestArtifact: null,
        generation: null,
        sectionNotes: [],
      },
    });
    const liveDoc = { ...TEST_DOCUMENT, name: "live-session" };
    const liveLoad = vi.fn(async () => ({ ...liveDoc }));
    const liveHost: DesignHost = { loadDocument: liveLoad };
    const droppedLoad = vi.fn(async () => ({ ...TEST_DOCUMENT }));
    const droppedHost: DesignHost = { loadDocument: droppedLoad };
    let resolveHost!: (host: DesignHost) => void;
    const hostPromise = new Promise<DesignHost>((resolve) => {
      resolveHost = resolve;
    });
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1", "Delegated"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
      ensureHost: () => hostPromise,
    });
    await flush();
    captured[0].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    await flush();

    // The human opens Design while the factory is still resolving.
    useAppStore.getState().setDesignHost(liveHost);
    useAppStore.getState().setDesignDocument(liveHost, liveDoc, []);
    resolveHost(droppedHost);
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.host).toBe(liveHost);
    expect(session.document).toMatchObject({ name: "live-session" });
    expect(session.latestArtifact).toMatchObject({ html: "<main>Delegated</main>" });
    expect(session.messages).toHaveLength(1);
    expect(droppedLoad).not.toHaveBeenCalled();
    expect(liveLoad).not.toHaveBeenCalled();
  });

  it("appends to a document that appears while loading its own", async () => {
    // The surface's own load lands while the mirror is still loading: the
    // mirror must append to the landed document, not replace it with the
    // one it loaded.
    const surfaceDoc = { ...TEST_DOCUMENT, name: "surface-loaded" };
    let resolveLoad!: (document: typeof TEST_DOCUMENT) => void;
    const loadPromise = new Promise<typeof TEST_DOCUMENT>((resolve) => {
      resolveLoad = resolve;
    });
    const loadingHost: DesignHost = { loadDocument: () => loadPromise };
    useAppStore.setState({
      designSession: {
        host: loadingHost,
        document: null,
        messages: [],
        latestArtifact: null,
        generation: null,
        sectionNotes: [],
      },
    });
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1", "Delegated"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
    });
    await flush();
    captured[0].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    await flush();

    // The surface load lands first, with an empty transcript.
    useAppStore.getState().setDesignDocument(loadingHost, surfaceDoc, []);
    resolveLoad({ ...TEST_DOCUMENT });
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.document).toMatchObject({ name: "surface-loaded" });
    expect(session.latestArtifact).toMatchObject({ html: "<main>Delegated</main>" });
    expect(session.messages).toHaveLength(1);
  });

  it("writes one card for two concurrent deliveries of the same finish", async () => {
    // A live finish plus its journal replay landing inside a load: both
    // runners pass the pre-replay checks, so idempotence must hold at the
    // write — same card id, same React key, written once.
    let resolveLoad!: (document: typeof TEST_DOCUMENT) => void;
    const loadPromise = new Promise<typeof TEST_DOCUMENT>((resolve) => {
      resolveLoad = resolve;
    });
    const loadingHost: DesignHost = { loadDocument: () => loadPromise };
    useAppStore.setState({
      designSession: {
        host: loadingHost,
        document: null,
        messages: [],
        latestArtifact: null,
        generation: null,
        sectionNotes: [],
      },
    });
    const captured: CapturedOpen[] = [];
    const deps = { openHistory: fakeOpen(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(childFinished("child-1", "Delegated"), deps);
    scheduleDelegatedDesignMirror(childFinished("child-1", "Delegated"), deps);
    await flush();
    expect(captured).toHaveLength(2);
    captured[0].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    captured[1].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    await flush();
    resolveLoad({ ...TEST_DOCUMENT });
    await flush();

    const cards = useAppStore
      .getState()
      .designSession.messages.filter((message) => message.id === "delegated-design-child-1");
    expect(cards).toHaveLength(1);
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>Delegated</main>",
    });
  });

  it("retries a child whose write found nothing to do", async () => {
    // The boolean slot: a pin landing mid-replay discards the write without
    // marking it mirrored, so the same finish presented again afterwards
    // still lands.
    const captured: CapturedOpen[] = [];
    const deps = { openHistory: fakeOpen(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(childFinished("child-1", "Delegated"), deps);
    await flush();
    noteHumanOpenedHistory("old-entry");
    captured[0].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    await flush();
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);

    clearDelegatedMirrorPin();
    scheduleDelegatedDesignMirror(childFinished("child-1", "Delegated"), deps);
    await flush();
    expect(captured).toHaveLength(2);
    captured[1].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    await flush();
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>Delegated</main>",
    });
  });

  it("holds a late artifact on a fresh host after the session was cleared", async () => {
    // The release interleave: the session is cleared (host null) while the
    // replay is in flight. The artifact still lands — on a fresh host, as a
    // complete session the next load stands down on — which is the retained
    // worked-host lifecycle, not a leak.
    const captured: CapturedOpen[] = [];
    scheduleDelegatedDesignMirror(childFinished("child-1", "Delegated"), {
      openHistory: fakeOpen(captured),
      store: useAppStore,
      ensureHost: async () => ({ loadDocument: async () => ({ ...TEST_DOCUMENT }) }),
    });
    await flush();
    useAppStore.getState().clearDesignSession(HOST);
    captured[0].onResult({ status: "artifact", html: "<main>Delegated</main>" });
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.host).not.toBeNull();
    expect(session.host).not.toBe(HOST);
    expect(session.document).not.toBeNull();
    expect(session.latestArtifact).toMatchObject({ html: "<main>Delegated</main>" });
    expect(session.messages).toHaveLength(1);
  });
});
