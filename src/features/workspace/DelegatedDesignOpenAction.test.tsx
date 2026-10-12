// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { useAppStore } from "../../store/appStore";
import type { DesignHost, DesignMessage } from "../design/designHost";
import { delegatedDesignCardId } from "../design/delegatedDesignMirror";
import { DelegatedDesignOpenAction } from "./DelegatedDesignOpenAction";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const HOST: DesignHost = {
  loadDocument: async () => ({ ...TEST_DOCUMENT }),
};

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

function setCanvas(messages: DesignMessage[]): void {
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
  useAppStore.getState().setDesignDocument(HOST, { ...TEST_DOCUMENT }, messages);
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

async function unmount(): Promise<void> {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
}

afterEach(unmount);

beforeEach(() => {
  setCanvas([mirroredCard("child-a", "<main>A</main>"), mirroredCard("child-b", "<main>B</main>")]);
});

async function render(childSessionId: string): Promise<void> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () =>
    root?.render(<DelegatedDesignOpenAction childSessionId={childSessionId} />),
  );
}

function button(): HTMLButtonElement | null {
  return host.querySelector<HTMLButtonElement>("button");
}

describe("the Open in Design action on a finished Design child", () => {
  it("is offered for a child whose card the canvas holds", async () => {
    await render("child-a");

    expect(button()?.textContent).toBe("Open in Design");
  });

  it("is not offered for a child the canvas does not hold", async () => {
    await render("child-x");

    expect(button()).toBeNull();
  });

  it("appears when the card lands after the line was drawn", async () => {
    setCanvas([]);
    await render("child-c");
    expect(button()).toBeNull();

    await act(async () => {
      useAppStore.getState().setDesignMessages(HOST, [mirroredCard("child-c", "<main>C</main>")]);
    });

    expect(button()?.textContent).toBe("Open in Design");
  });

  it("opens Design on that child's artifact when pressed", async () => {
    await render("child-a");

    await act(async () => button()?.click());

    expect(useAppStore.getState().activeSurface).toBe("design");
    expect(useAppStore.getState().designSession.latestArtifact).toMatchObject({
      html: "<main>A</main>",
    });
  });
});
