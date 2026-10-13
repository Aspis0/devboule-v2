import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "../../store/appStore";
import type { DesignHost, DesignMessage } from "./designHost";
import type { DesignPreviewBlockMessage } from "./DesignPreviewBlock";
import { delegatedDesignCardId } from "./delegatedDesignMirror";
import { openDesignResult } from "./designResultOpen";

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

/** A reply the surface wrote for a direct run, which names no child session. */
function directCard(id: string, html: string): DesignPreviewBlockMessage {
  return {
    id,
    role: "assistant",
    status: "done",
    title: id,
    desc: "",
    sources: [],
    nodeIds: [],
    artifactHtml: html,
  };
}

function mirroredCard(childSessionId: string, html: string): DesignMessage {
  return {
    id: delegatedDesignCardId(childSessionId),
    role: "assistant",
    status: "done",
    title: childSessionId,
    desc: "Finished by a Design agent.",
    sources: [],
    nodeIds: [],
    artifactHtml: html,
  } as DesignMessage;
}

function setSession(messages: DesignMessage[], generating = false): void {
  useAppStore.setState({
    activeSurface: "workspace",
    designSession: {
      host: HOST,
      document: null,
      messages: [],
      latestArtifact: null,
      generation: null,
      sectionNotes: [],
      lastCanvasPageHeight: null,
      mirroredFitPending: false,
    },
  });
  // Through the store's own writers, so latestArtifact is derived as it is in production.
  const store = useAppStore.getState();
  store.setDesignDocument(HOST, { ...TEST_DOCUMENT }, messages);
  store.setDesignGeneration(
    HOST,
    generating ? { assistantId: "live", controller: new AbortController() } : null,
  );
}

function cardIds(): string[] {
  return useAppStore.getState().designSession.messages.map((message) => message.id);
}

beforeEach(() => {
  setSession([
    mirroredCard("child-a", "<main>A</main>"),
    directCard("assistant-1", "<main>Direct</main>"),
  ]);
});

describe("opening a direct run's Design result", () => {
  it("moves that result to the end, so the canvas opens on it", () => {
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>Direct</main>",
    });

    openDesignResult(directCard("assistant-1", "<main>Direct</main>"));

    expect(cardIds()).toEqual([delegatedDesignCardId("child-a"), "assistant-1"]);
    expect(useAppStore.getState().activeSurface).toBe("design");
  });

  it("moves one result to the end and leaves the rest of the transcript in order", () => {
    setSession([
      mirroredCard("child-a", "<main>A</main>"),
      directCard("assistant-1", "<main>Direct</main>"),
      directCard("assistant-2", "<main>Second</main>"),
    ]);

    openDesignResult(directCard("assistant-1", "<main>Direct</main>"));

    expect(cardIds()).toEqual([delegatedDesignCardId("child-a"), "assistant-2", "assistant-1"]);
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>Direct</main>",
    });
    expect(useAppStore.getState().designSession.mirroredFitPending).toBe(true);
  });

  it("keeps the transcript untouched and asks for no fit while a generation runs", () => {
    setSession(
      [mirroredCard("child-a", "<main>A</main>"), directCard("assistant-2", "<main>B</main>")],
      true,
    );

    openDesignResult(directCard("assistant-2", "<main>B</main>"));

    expect(cardIds()).toEqual([delegatedDesignCardId("child-a"), "assistant-2"]);
    expect(useAppStore.getState().designSession.mirroredFitPending).toBe(false);
    expect(useAppStore.getState().activeSurface).toBe("workspace");
  });

  it("does nothing when Design has no host to open", () => {
    setSession([directCard("assistant-1", "<main>Direct</main>")]);
    useAppStore.setState({
      designSession: { ...useAppStore.getState().designSession, host: null },
    });

    openDesignResult(directCard("assistant-1", "<main>Direct</main>"));

    expect(cardIds()).toEqual(["assistant-1"]);
    expect(useAppStore.getState().activeSurface).toBe("workspace");
  });

  it("does not replace a live card with the caller's older copy of it", () => {
    setSession([
      directCard("assistant-1", "<main>Direct</main>"),
      directCard("assistant-2", "<main>Second</main>"),
    ]);

    openDesignResult(directCard("assistant-1", "<main>Stale</main>"));

    // The card that moved is the one the transcript holds, so a caller holding an older
    // copy cannot put its own page into the canvas.
    expect(useAppStore.getState().designSession.messages.at(-1)).toMatchObject({
      id: "assistant-1",
      artifactHtml: "<main>Direct</main>",
    });
    expect(useAppStore.getState().activeSurface).toBe("design");
  });
});
