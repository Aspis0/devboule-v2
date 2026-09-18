// @vitest-environment happy-dom

import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

// The property test below runs the mirror through its real default path,
// so the daemon door it knocks on is the mocked `invoke`, not an injected
// seam: whatever the module asks for lands here by command name.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(),
  Channel: class Channel {},
}));

import { invoke } from "@tauri-apps/api/core";
import { useAppStore } from "../../store/appStore";
import type { SessionEvent } from "../../types/ipc";
import type { FinishArtifactPart } from "../../types/ipc";
import type { AttachmentReference, StoredAttachment } from "../../lib/tauri";
import type { DesignMessage } from "./designHost";
import type { DesignHost } from "./designHost";
import {
  clearDelegatedMirrorPin,
  noteHumanOpenedHistory,
  resetDelegatedMirrorForTests,
  scheduleDelegatedDesignMirror,
  type DelegatedMirrorDeps,
} from "./delegatedDesignMirror";

type ChildFinishedEvent = Extract<SessionEvent, { type: "child_finished" }>;

interface CapturedRead {
  reference: AttachmentReference;
  resolve: (stored: StoredAttachment) => void;
  reject: (error: unknown) => void;
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

const DIGEST = "a".repeat(64);

function markdownPart(url: string, storedBytes = 512): FinishArtifactPart {
  return { url, mimeType: "text/markdown", metadata: { storedBytes } };
}

function childFinished(
  childSessionId: string,
  displayName = "design child",
  state: ChildFinishedEvent["state"] = "completed",
  artifacts: ChildFinishedEvent["artifacts"] = [],
): ChildFinishedEvent {
  return {
    type: "child_finished",
    messageId: "m1",
    childSessionId,
    displayName,
    state,
    artifacts,
  };
}

/** A finish whose first markdown part names real stored bytes. */
function finishedWithMarkdown(
  childSessionId: string,
  displayName = "design child",
): ChildFinishedEvent {
  return childFinished(childSessionId, displayName, "completed", [
    {
      artifactId: `devboule-attachment:s.creator.1/${DIGEST}`,
      parts: [markdownPart(`devboule-attachment:s.creator.1/${DIGEST}`)],
    },
  ]);
}

function toBase64(text: string): string {
  return Buffer.from(text, "utf8").toString("base64");
}

function storedMarkdown(markdown: string): StoredAttachment {
  return { mimeType: "text/markdown", data: toBase64(markdown) };
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

function fakeRead(captured: CapturedRead[]): DelegatedMirrorDeps["readStored"] {
  return ((reference: AttachmentReference) =>
    new Promise<StoredAttachment>((resolve, reject) => {
      captured.push({ reference, resolve, reject });
    })) as DelegatedMirrorDeps["readStored"];
}

async function flush(): Promise<void> {
  for (let index = 0; index < 20; index += 1) await Promise.resolve();
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
  vi.useRealTimers();
});

// The mirror resolves its extractor through a dynamic import, so the first
// schedule in this file pays the module load. Pre-warm it once: without
// this the test flushes would wait on module loading instead of the mirror.
beforeAll(async () => {
  await import("./agentHost");
});

describe("delegated design mirror", () => {
  it("reads the artifact's bytes and writes the extracted card", async () => {
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Landing page"), {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();
    expect(captured).toHaveLength(1);
    expect(captured[0].reference).toEqual({
      sessionId: "s.creator.1",
      digest: DIGEST,
      storedBytes: 512,
    });

    captured[0].resolve(storedMarkdown("Intro.\n```html\n<main>Delegated</main>\n```\nOutro."));
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toMatchObject({ html: "<main>Delegated</main>" });
    expect(session.messages).toHaveLength(1);
    const message = session.messages[0];
    expect(message?.id).toBe("delegated-design-child-1");
    expect(message?.role).toBe("assistant");
    if (message?.role === "assistant") {
      expect(message.artifactHtml).toBe("<main>Delegated</main>");
      expect(message.title).toBe("Landing page");
    }
  });

  it("asks the daemon for nothing but the attachment read", async () => {
    // The header's guarantee as a property, not an inventory: the mirror
    // runs its real default path here — no injected read seam — with the
    // daemon door mocked, so a second command added tomorrow fails this
    // test whatever it is.
    vi.mocked(invoke).mockClear();
    vi.mocked(invoke).mockResolvedValue({
      mimeType: "text/markdown",
      data: toBase64("```html\n<main>Only</main>\n```"),
    } as never);
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1"), {
      store: useAppStore,
    });
    await flush();
    await flush();
    expect(vi.mocked(invoke).mock.calls.map(([command]) => command)).toEqual([
      "session_attachment_read",
    ]);
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>Only</main>",
    });
  });

  it("writes nothing for a part whose url does not parse, without reading", async () => {
    const captured: CapturedRead[] = [];
    const event = childFinished("child-1", "design child", "completed", [
      {
        artifactId: "not-a-reference",
        parts: [{ url: "not-a-reference", mimeType: "text/markdown" }],
      },
    ]);
    scheduleDelegatedDesignMirror(event, {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();

    // Unparseable means unread: no daemon call, no card, nothing thrown.
    expect(captured).toHaveLength(0);
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toBeNull();
    expect(session.messages).toHaveLength(0);
  });

  it("writes nothing when no part is markdown, without reading", async () => {
    const captured: CapturedRead[] = [];
    const event = childFinished("child-1", "design child", "completed", [
      {
        artifactId: `devboule-attachment:s.creator.1/${DIGEST}`,
        parts: [
          {
            url: `devboule-attachment:s.creator.1/${DIGEST}`,
            mimeType: "image/png",
          },
        ],
      },
    ]);
    scheduleDelegatedDesignMirror(event, {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();

    // A part of another type is not markdown and must not be extracted from.
    expect(captured).toHaveLength(0);
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);
  });

  it("lets the first markdown part decide, even when a later one would parse", async () => {
    const captured: CapturedRead[] = [];
    const event = childFinished("child-1", "design child", "completed", [
      {
        artifactId: "junk-then-real",
        parts: [
          { url: "junk", mimeType: "text/markdown" },
          markdownPart(`devboule-attachment:s.creator.1/${DIGEST}`),
        ],
      },
    ]);
    scheduleDelegatedDesignMirror(event, {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();

    expect(captured).toHaveLength(0);
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);
  });

  it("writes nothing when the stored markdown holds no fenced HTML", async () => {
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1"), {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();
    captured[0].resolve(storedMarkdown("Just prose, no design block."));
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toBeNull();
    expect(session.messages).toHaveLength(0);
  });

  it("keeps non-ASCII markdown intact through the decode", async () => {
    // `atob` yields one byte per character: decoding its output as text
    // corrupts every accented byte into mojibake ("CafÃ©"). Bytes first,
    // then TextDecoder, keeps the prose the child wrote.
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1"), {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();
    captured[0].resolve(
      storedMarkdown("Café — naïve façade.\n```html\n<p>crème brûlée</p>\n```\n"),
    );
    await flush();

    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<p>crème brûlée</p>",
    });
  });

  it("warns and writes nothing when the read fails", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      const captured: CapturedRead[] = [];
      scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1"), {
        readStored: fakeRead(captured),
        store: useAppStore,
      });
      await flush();
      captured[0].reject(new Error("attachment swept"));
      await flush();

      expect(warn).toHaveBeenCalled();
      const session = useAppStore.getState().designSession;
      expect(session.latestArtifact).toBeNull();
      expect(session.messages).toHaveLength(0);
    } finally {
      warn.mockRestore();
    }
  });

  it("stays silent when the finish carries no artifacts", async () => {
    // The common case for a child whose job was never design: the event
    // carries a note saying why, and the mirror must not read or write.
    const captured: CapturedRead[] = [];
    const event: ChildFinishedEvent = {
      ...childFinished("child-1"),
      note: "the child produced no design artifact",
    };
    scheduleDelegatedDesignMirror(event, {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();

    expect(captured).toHaveLength(0);
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toBeNull();
    expect(session.messages).toHaveLength(0);
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

    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("new-child", "Newer design"), {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();

    // Pinned means not even read: no daemon call for the newer child.
    expect(captured).toHaveLength(0);
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toMatchObject({ html: "<main>Old</main>" });
    expect(session.messages).toHaveLength(1);

    // The pin is the human's to release: after it clears the same arrival mirrors.
    clearDelegatedMirrorPin();
    scheduleDelegatedDesignMirror(finishedWithMarkdown("new-child", "Newer design"), {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();
    expect(captured).toHaveLength(1);
  });

  it("writes nothing when the human is already viewing that exact child", async () => {
    noteHumanOpenedHistory("child-1");
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1"), {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();

    expect(captured).toHaveLength(0);
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);
  });

  it("ignores a child whose state is not completed, even with an artifact", async () => {
    // A canceled child can carry an artifact from its dead run; the panel
    // must not present it as finished work.
    const captured: CapturedRead[] = [];
    const canceled: ChildFinishedEvent = {
      ...finishedWithMarkdown("child-1", "Canceled work"),
      state: "canceled",
    };
    scheduleDelegatedDesignMirror(canceled, {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();

    // Not even read: the state alone disqualifies the arrival.
    expect(captured).toHaveLength(0);
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toBeNull();
    expect(session.messages).toHaveLength(0);
  });

  it("shows the design arrival when a later arrival has nothing to show", async () => {
    // The exact interleave the sequence guard must survive: A carries design
    // but reads slowly, B finishes later with prose and no fence. B must not
    // consume the slot — "latest wins" means the latest arrival that has
    // something to show.
    const captured: CapturedRead[] = [];
    const deps = { readStored: fakeRead(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-a", "Design work"), deps);
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-b", "Coder work"), deps);
    await flush();
    expect(captured).toHaveLength(2);

    // The coder's read resolves first, with no fence: the panel is untouched.
    captured[1].resolve(storedMarkdown("Just prose."));
    await flush();
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);

    // The design arrival resolves after — and still lands.
    captured[0].resolve(storedMarkdown("```html\n<main>A</main>\n```"));
    await flush();
    const session = useAppStore.getState().designSession;
    expect(session.latestArtifact).toMatchObject({ html: "<main>A</main>" });
    expect(session.messages).toHaveLength(1);
  });

  it("lets the later arrival win when two children finish close together", async () => {
    const captured: CapturedRead[] = [];
    const deps = { readStored: fakeRead(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-a", "First"), deps);
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-b", "Second"), deps);
    await flush();
    expect(captured).toHaveLength(2);

    captured[1].resolve(storedMarkdown("```html\n<main>B</main>\n```"));
    await flush();
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>B</main>",
    });

    // The earlier arrival's slow read is stale and must not overwrite the newer one.
    captured[0].resolve(storedMarkdown("```html\n<main>A</main>\n```"));
    await flush();
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>B</main>",
    });
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
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "First delegation"), {
      readStored: fakeRead(captured),
      store: useAppStore,
      // The production default dynamic-imports the real host factory; the
      // import's own ticks would make this test wait on module loading
      // instead of on the mirror, so the factory is injected and the test
      // asserts what the mirror does with the host it is given.
      ensureHost: async () => freshHost,
    });
    await flush();
    captured[0].resolve(storedMarkdown("```html\n<main>First</main>\n```"));
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
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Delegated"), {
      readStored: fakeRead(captured),
      store: useAppStore,
      ensureHost: () => hostPromise,
    });
    await flush();
    captured[0].resolve(storedMarkdown("```html\n<main>Delegated</main>\n```"));
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
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Delegated"), {
      readStored: fakeRead(captured),
      store: useAppStore,
    });
    await flush();
    captured[0].resolve(storedMarkdown("```html\n<main>Delegated</main>\n```"));
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
    // runners pass the pre-read checks, so idempotence must hold at the
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
    const captured: CapturedRead[] = [];
    const deps = { readStored: fakeRead(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Delegated"), deps);
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Delegated"), deps);
    await flush();
    expect(captured).toHaveLength(2);
    captured[0].resolve(storedMarkdown("```html\n<main>Delegated</main>\n```"));
    captured[1].resolve(storedMarkdown("```html\n<main>Delegated</main>\n```"));
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
    // The boolean slot: a pin landing mid-read discards the write without
    // marking it mirrored, so the same finish presented again afterwards
    // still lands.
    const captured: CapturedRead[] = [];
    const deps = { readStored: fakeRead(captured), store: useAppStore };
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Delegated"), deps);
    await flush();
    noteHumanOpenedHistory("old-entry");
    captured[0].resolve(storedMarkdown("```html\n<main>Delegated</main>\n```"));
    await flush();
    expect(useAppStore.getState().designSession.messages).toHaveLength(0);

    clearDelegatedMirrorPin();
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Delegated"), deps);
    await flush();
    expect(captured).toHaveLength(2);
    captured[1].resolve(storedMarkdown("```html\n<main>Delegated</main>\n```"));
    await flush();
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>Delegated</main>",
    });
  });

  it("holds a late artifact on a fresh host after the session was cleared", async () => {
    // The release interleave: the session is cleared (host null) while the
    // read is in flight. The artifact still lands — on a fresh host, as a
    // complete session the next load stands down on — which is the retained
    // worked-host lifecycle, not a leak.
    const captured: CapturedRead[] = [];
    scheduleDelegatedDesignMirror(finishedWithMarkdown("child-1", "Delegated"), {
      readStored: fakeRead(captured),
      store: useAppStore,
      ensureHost: async () => ({ loadDocument: async () => ({ ...TEST_DOCUMENT }) }),
    });
    await flush();
    useAppStore.getState().clearDesignSession(HOST);
    captured[0].resolve(storedMarkdown("```html\n<main>Delegated</main>\n```"));
    await flush();

    const session = useAppStore.getState().designSession;
    expect(session.host).not.toBeNull();
    expect(session.host).not.toBe(HOST);
    expect(session.document).not.toBeNull();
    expect(session.latestArtifact).toMatchObject({ html: "<main>Delegated</main>" });
    expect(session.messages).toHaveLength(1);
  });
});
