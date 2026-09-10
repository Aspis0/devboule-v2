// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useAppStore, type DesignSessionState } from "../../store/appStore";
import type { DesignHost } from "./designHost";
import { DesignPreviewPanel } from "./DesignPreviewPanel";

const NOT_OPENED_MESSAGE = "Design has not been opened in this session yet";
const NOTHING_GENERATED_MESSAGE = "Design is open, but nothing has been generated yet.";
const GENERATING_MESSAGE = "Generating a design…";
const REJECTED_MESSAGE = "The last artifact was rejected.";
// A payload distinctive enough that rendering it anywhere would be caught.
const ARTIFACT_MARKUP = '<p data-probe="artifact">ARTIFACT MARKUP PAYLOAD</p>';
const MOCKUP_CARD_TITLE = "Edited Index header";

const HOST: DesignHost = {
  loadDocument: async () => {
    throw new Error("not used in this test");
  },
};

function session(overrides: Partial<DesignSessionState> = {}): {
  designSession: DesignSessionState;
} {
  return {
    designSession: {
      host: HOST,
      document: null,
      messages: [],
      latestArtifact: null,
      generation: null,
      sectionNotes: [],
      ...overrides,
    },
  };
}

describe("DesignPreviewPanel", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    useAppStore.setState({ activeSurface: "workspace" });
    useAppStore.setState(session());
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    useAppStore.setState(session());
  });

  async function renderPanel(): Promise<void> {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root.render(<DesignPreviewPanel />);
    });
  }

  it("says Design was never opened when host is null, not that nothing was generated", async () => {
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

    await renderPanel();

    expect(container.textContent).toContain(NOT_OPENED_MESSAGE);
    expect(container.textContent).not.toContain(NOTHING_GENERATED_MESSAGE);
  });

  it("announces a running generation", async () => {
    useAppStore.setState(
      session({ generation: { assistantId: "assistant-1", controller: new AbortController() } }),
    );

    await renderPanel();

    expect(container.textContent).toContain(GENERATING_MESSAGE);
    // A running generation is still not finished work: no artifact claims appear.
    expect(container.textContent).not.toContain(NOTHING_GENERATED_MESSAGE);
  });

  it("renders an artifact error and keeps it distinct from having no artifact", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { error: "Artifact exceeds the size cap." },
        // Store-reachable state: this message is what the store's latestArtifact predicate
        // would derive the injected value from (artifactError set, no artifactHtml).
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            status: "done",
            title: "Reopened design",
            desc: "The reopened artifact could not be displayed.",
            sources: [],
            nodeIds: [],
            artifactError: "Artifact exceeds the size cap.",
          },
        ],
      }),
    );

    await renderPanel();

    expect(container.textContent).toContain(REJECTED_MESSAGE);
    expect(container.textContent).toContain("Artifact exceeds the size cap.");
    expect(container.textContent).not.toContain(NOTHING_GENERATED_MESSAGE);
  });

  it("mirrors the store artifact as a truthful placeholder, never as rendered HTML", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { html: ARTIFACT_MARKUP },
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            status: "done",
            title: "Edited the card",
            desc: "Snapped the radius to the token.",
            sources: ["src/real/source.tsx"],
            nodeIds: ["node-1"],
            // The store derives latestArtifact from this field, so a store-consistent
            // state must carry it on the message too (see the pairing test below).
            artifactHtml: ARTIFACT_MARKUP,
          },
        ],
      }),
    );

    await renderPanel();

    expect(container.textContent).toContain("Edited the card");
    expect(container.textContent).toContain("Snapped the radius to the token.");
    expect(container.textContent).toContain("src/real/source.tsx");
    expect(container.textContent).toContain("a scaled preview here is next");
    // The prohibition, asserted: no iframe and no artifact markup anywhere in the DOM.
    expect(container.querySelectorAll("iframe")).toHaveLength(0);
    expect(container.innerHTML).not.toContain("ARTIFACT MARKUP PAYLOAD");
    expect(container.innerHTML).not.toContain("<iframe");
  });

  it("says nothing has been generated when Design is open but empty", async () => {
    await renderPanel();

    expect(container.textContent).toContain(NOTHING_GENERATED_MESSAGE);
    expect(container.textContent).not.toContain(NOT_OPENED_MESSAGE);
    expect(container.textContent).not.toContain(REJECTED_MESSAGE);
  });

  it("renders no iframe in any state", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { html: ARTIFACT_MARKUP, error: "Artifact exceeds the size cap." },
        // Store-reachable state: the injected latestArtifact matches this message's own
        // artifact fields, as the store's predicate would produce it.
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            status: "done",
            title: "Reopened design",
            desc: "The reopened artifact could not be displayed.",
            sources: [],
            nodeIds: [],
            artifactHtml: ARTIFACT_MARKUP,
            artifactError: "Artifact exceeds the size cap.",
          },
        ],
        generation: { assistantId: "assistant-1", controller: new AbortController() },
      }),
    );

    await renderPanel();

    expect(container.querySelectorAll("iframe")).toHaveLength(0);
  });

  it("carries no hardcoded string from the old mockup panel", async () => {
    await renderPanel();

    expect(container.textContent).not.toContain(MOCKUP_CARD_TITLE);
    expect(container.textContent).not.toContain("Mockup");
    expect(container.textContent).not.toContain("1 generation");
  });

  // Regression pin for the card/message pairing: the artifact branch must describe the
  // message the artifact came from (the same predicate as `latestArtifact` in
  // src/store/appStore.ts), not merely the last settled reply.
  it("pairs the artifact card with the message the artifact came from, not the last settled reply", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { html: ARTIFACT_MARKUP },
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            status: "done",
            title: "Made a card",
            desc: "Built the card.",
            sources: ["src/card/source.tsx"],
            nodeIds: ["node-1"],
            artifactHtml: ARTIFACT_MARKUP,
          },
          {
            id: "assistant-2",
            role: "assistant",
            status: "done",
            title: "Tokens used",
            desc: "Listed the tokens.",
            sources: ["src/tokens/source.ts"],
            nodeIds: ["node-2"],
          },
        ],
      }),
    );

    await renderPanel();

    expect(container.textContent).toContain("Made a card");
    expect(container.textContent).toContain("Built the card.");
    expect(container.textContent).toContain("src/card/source.tsx");
    expect(container.textContent).not.toContain("Tokens used");
    expect(container.textContent).not.toContain("Listed the tokens.");
    expect(container.textContent).not.toContain("src/tokens/source.ts");
  });

  // Error wins over a renderable artifact: reversing the two branches would otherwise
  // present a rejected artifact as if it had rendered.
  it("renders the error branch, not the artifact card, when both html and error are set", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { html: ARTIFACT_MARKUP, error: "Artifact exceeds the size cap." },
        // Store-reachable state: the injected latestArtifact matches this message's own
        // artifact fields, as the store's predicate would produce it.
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            status: "done",
            title: "Reopened design",
            desc: "The reopened artifact could not be displayed.",
            sources: [],
            nodeIds: [],
            artifactHtml: ARTIFACT_MARKUP,
            artifactError: "Artifact exceeds the size cap.",
          },
        ],
      }),
    );

    await renderPanel();

    expect(container.textContent).toContain(REJECTED_MESSAGE);
    expect(container.textContent).toContain("Artifact exceeds the size cap.");
    expect(container.textContent).not.toContain("a scaled preview here is next");
    expect(container.innerHTML).not.toContain("ARTIFACT MARKUP PAYLOAD");
  });

  it("renders a settled reply with no artifact as its own card", async () => {
    useAppStore.setState(
      session({
        latestArtifact: null,
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            status: "done",
            title: "Tokens used",
            desc: "Listed the tokens.",
            sources: ["src/tokens/source.ts"],
            nodeIds: ["node-2"],
          },
        ],
      }),
    );

    await renderPanel();

    expect(container.textContent).toContain("Tokens used");
    expect(container.textContent).toContain("Listed the tokens.");
    // No artifact exists, so no artifact-card framing may appear.
    expect(container.textContent).not.toContain("a scaled preview here is next");
    expect(container.textContent).not.toContain("The last artifact was rejected.");
  });

  it("keeps Open Design working and points it at the design surface", async () => {
    await renderPanel();

    const openDesign = container.querySelector<HTMLButtonElement>(".workspace-open-design");
    if (openDesign === null) throw new Error("Open Design button did not render");
    await act(async () => {
      openDesign.click();
    });
    expect(useAppStore.getState().activeSurface).toBe("design");
  });
});
