import { beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "../../store/appStore";
import type { DesignHost, DesignMessage } from "./designHost";
import { delegatedDesignCardId } from "./delegatedDesignMirror";
import { hasMirroredDesignCard, openMirroredDesign } from "./mirroredDesignOpen";

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
    mirroredCard("child-b", "<main>B</main>"),
  ]);
});

describe("opening a mirrored Design child", () => {
  it("puts that child's artifact on the canvas when a later child landed after it", () => {
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>B</main>",
    });

    openMirroredDesign("child-a");

    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>A</main>",
    });
    expect(useAppStore.getState().activeSurface).toBe("design");
    expect(cardIds()).toEqual([delegatedDesignCardId("child-b"), delegatedDesignCardId("child-a")]);
  });

  it("keeps the order when the child is already the latest", () => {
    openMirroredDesign("child-b");

    expect(cardIds()).toEqual([delegatedDesignCardId("child-a"), delegatedDesignCardId("child-b")]);
    expect(useAppStore.getState().activeSurface).toBe("design");
  });

  it("does nothing while a generation runs, so the canvas keeps the artifact it shows", () => {
    setSession(
      [mirroredCard("child-a", "<main>A</main>"), mirroredCard("child-b", "<main>B</main>")],
      true,
    );

    openMirroredDesign("child-a");

    expect(cardIds()).toEqual([delegatedDesignCardId("child-a"), delegatedDesignCardId("child-b")]);
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>B</main>",
    });
    expect(useAppStore.getState().activeSurface).toBe("workspace");
  });

  it("does nothing for a child the canvas does not hold", () => {
    openMirroredDesign("child-x");

    expect(cardIds()).toEqual([delegatedDesignCardId("child-a"), delegatedDesignCardId("child-b")]);
    expect(useAppStore.getState().activeSurface).toBe("workspace");
  });

  it("reports whether the canvas holds a child's card", () => {
    expect(hasMirroredDesignCard(useAppStore.getState().designSession.messages, "child-a")).toBe(
      true,
    );
    expect(hasMirroredDesignCard(useAppStore.getState().designSession.messages, "child-x")).toBe(
      false,
    );
  });
});
