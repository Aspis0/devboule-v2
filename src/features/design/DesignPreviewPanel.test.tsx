// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore, type DesignSessionState } from "../../store/appStore";
import type { DesignHost, DesignMessage } from "./designHost";
import { delegatedDesignCardId } from "./delegatedDesignMirror";
import { ARTIFACT_CSP_META } from "./artifactCsp";
import { ARTIFACT_PAGE_HEIGHT, ARTIFACT_PAGE_WIDTH, maxArtifactScroll } from "./artifactViewport";
import {
  getCachedArtifactStructure,
  setCachedArtifactStructure,
  clearCachedArtifactSections,
} from "./artifactStructure";
import {
  ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
  ARTIFACT_RENDER_CRITIC_SOURCE,
} from "./artifactRenderCritic";
import { DesignPreviewPanel } from "./DesignPreviewPanel";
import { ImageLightbox } from "../workspace/transcript/ImageLightbox";

const EMPTY_CANVAS_MESSAGE =
  "Nothing on the canvas yet. A Design agent's result appears here when it finishes.";
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

/** The transcript's own viewer, so a test can open one over the enlarged preview. */
function TransientImageLightbox() {
  const [open, setOpen] = useState(true);
  return open ? (
    <ImageLightbox
      images={[{ url: "blob:shot", alt: "Attached image 1 of 1" }]}
      index={0}
      openedIndex={0}
      openedUrl="blob:shot"
      onIndexChange={() => undefined}
      onClose={() => setOpen(false)}
    />
  ) : null;
}

// The preview's stylesheet, read as text. happy-dom does not lay out, so the half of the
// geometry that lives in CSS — which property the page box reads its height from, and
// that the preview box declares no height of its own — is only checkable here.
// designPills.test.ts reads design.css the same way.
const PREVIEW_CSS = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "artifactPreview.css"),
  "utf8",
);

const DESIGN_CSS = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "design.css"),
  "utf8",
);

function cssBlockFor(selector: string): string {
  const start = PREVIEW_CSS.indexOf(selector);
  if (start < 0) throw new Error(`Selector missing: ${selector}`);
  const open = PREVIEW_CSS.indexOf("{", start);
  const close = PREVIEW_CSS.indexOf("}", open);
  if (open < 0 || close < 0) throw new Error(`Block missing: ${selector}`);
  return PREVIEW_CSS.slice(open + 1, close);
}

/**
 * happy-dom 20 ships a ResizeObserver whose observe/unobserve/disconnect are documented
 * no-ops, so nothing in this environment will ever deliver a measured box. The panel's
 * observer is replaced with this one so a test can fire a delivery by hand and see what
 * the component wrote; the production code carries no branch for a missing observer.
 */
class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  observed: Element[] = [];
  disconnects = 0;
  private readonly callback: () => void;

  constructor(callback: () => void) {
    this.callback = callback;
    ResizeObserverStub.instances.push(this);
  }

  observe(target: Element): void {
    this.observed.push(target);
  }

  unobserve(): void {}

  disconnect(): void {
    this.disconnects += 1;
  }

  fire(): void {
    this.callback();
  }
}

function installResizeObserverStub(): () => void {
  const previous = globalThis.ResizeObserver;
  ResizeObserverStub.instances = [];
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver;
  return () => {
    globalThis.ResizeObserver = previous;
  };
}

/**
 * happy-dom does not lay out, so a box is 0x0 unless a test says otherwise — and the one
 * moment at which it is too late to say so is after the render: the panel measures from a
 * callback ref, which runs inside the commit, so it has already read the box by the time the
 * test holds the node. Overriding the metric on the element prototype makes the value
 * available from the moment the node exists; the returned function restores what was there.
 */
function stubBoxMetrics(width: number, height: number): () => void {
  const undo = [overrideBoxMetric("clientWidth", width), overrideBoxMetric("clientHeight", height)];
  return () => {
    for (const restore of undo) restore();
  };
}

function overrideBoxMetric(name: "clientWidth" | "clientHeight", value: number): () => void {
  const prototype = HTMLDivElement.prototype;
  const original = Object.getOwnPropertyDescriptor(prototype, name);
  Object.defineProperty(prototype, name, { configurable: true, get: () => value });
  return () => {
    if (original === undefined) Reflect.deleteProperty(prototype, name);
    else Object.defineProperty(prototype, name, original);
  };
}

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
      lastCanvasPageHeight: null,
      mirroredFitPending: false,
      ...overrides,
    },
  };
}

/**
 * A store-consistent one-result session. The panel reads the transcript rather than the
 * store's derivation of it, so a session that sets only `latestArtifact` is a state the
 * store cannot produce and renders no block: the html has to be on the message too.
 */
function artifactSession(
  html: string,
  overrides: Partial<DesignSessionState> = {},
): { designSession: DesignSessionState } {
  return session({
    latestArtifact: { html },
    messages: [
      {
        id: "assistant-1",
        role: "assistant",
        status: "done",
        title: "Edited the card",
        desc: "",
        sources: [],
        nodeIds: [],
        artifactHtml: html,
      },
    ],
    ...overrides,
  });
}

/** What the render critic posts back from its off-screen measurement frame. */
function criticResult(contentHeight: number): Record<string, unknown> {
  return {
    kind: ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
    source: ARTIFACT_RENDER_CRITIC_SOURCE,
    version: 1,
    findings: [],
    structure: [],
    contentHeight,
  };
}

function shiftWheel(deltaY: number): WheelEvent {
  const wheel = new Event("wheel", { bubbles: true, cancelable: true }) as WheelEvent;
  Object.defineProperties(wheel, {
    deltaY: { value: deltaY },
    deltaMode: { value: 0 },
    shiftKey: { value: true },
  });
  return wheel;
}

function press(key: string) {
  document.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
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

  it("says the canvas is empty when host is null, not that nothing was generated", async () => {
    useAppStore.setState({
      designSession: {
        host: null,
        document: null,
        messages: [],
        latestArtifact: null,
        generation: null,
        sectionNotes: [],
        lastCanvasPageHeight: null,
        mirroredFitPending: false,
      },
    });

    await renderPanel();

    expect(container.textContent).toContain(EMPTY_CANVAS_MESSAGE);
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

  it("renders the store artifact as a scaled preview frame under the artifact policy", async () => {
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
            // state must carry it on the message too (see the branch test below).
            artifactHtml: ARTIFACT_MARKUP,
          },
        ],
      }),
    );

    await renderPanel();

    // The header row is a control, not commentary, and it is what surrounds the preview:
    // without it the panel would strand the user with no way back to the canvas.
    expect(container.textContent).toContain("Live preview");
    expect(container.querySelector(".workspace-open-design")).not.toBeNull();
    // The page and the agent's name for it. This branch used to carry the artifact
    // message's description and source chips as well; the card is the frame around the
    // preview, so none of that text may reach it.
    expect(container.querySelector(".design-preview-card")).not.toBeNull();
    expect(container.textContent).toContain("Edited the card");
    expect(container.textContent).not.toContain("Snapped the radius to the token.");
    expect(container.textContent).not.toContain("src/real/source.tsx");

    const frames = container.querySelectorAll<HTMLIFrameElement>("iframe");
    expect(frames).toHaveLength(1);
    const frame = frames[0];
    // The canvas frame, not a second renderer: same empty sandbox, and the srcdoc
    // carries the artifact behind the one confinement policy.
    expect(frame.getAttribute("sandbox")).toBe("");
    const srcDoc = frame.getAttribute("srcdoc") ?? "";
    // The policy is delivered inside the frame and ahead of the markup, the same
    // order the canvas uses: a `<meta>` CSP only governs what follows it.
    expect(srcDoc.startsWith(`${ARTIFACT_CSP_META}\n`)).toBe(true);
    expect(srcDoc).toContain(ARTIFACT_MARKUP);
    // A preview is a picture: it stays out of the pointer path.
    expect(frame.style.pointerEvents).toBe("none");
    // …and out of the focus path, which is the separate guarantee. `pointerEvents`
    // stops the mouse; only `inert` keeps Tab from walking into a generated page's
    // links and fields. The canvas carries it on its own wrapper for the same reason,
    // so asserting it here is what keeps the two from drifting apart.
    expect(frame.closest(".design-artifact-preview")?.hasAttribute("inert")).toBe(true);
    // The page lays out in a box sized from the column, so a document taller than that
    // box must not be able to grow a scrollbar inside the thumbnail.
    expect(frame.getAttribute("scrolling")).toBe("no");
    // The markup lives in the frame's attribute and nowhere else: the panel's own
    // document still renders none of it.
    expect(container.textContent).not.toContain("ARTIFACT MARKUP PAYLOAD");
    // The scaled page box is what fits a 1280px page into a narrow panel.
    expect(frame.parentElement?.className).toContain("design-artifact-preview-page");
  });

  it("says nothing has been generated when Design is open but empty", async () => {
    await renderPanel();

    expect(container.textContent).toContain(NOTHING_GENERATED_MESSAGE);
    expect(container.textContent).not.toContain(EMPTY_CANVAS_MESSAGE);
    expect(container.textContent).not.toContain(REJECTED_MESSAGE);
  });

  it("states a rejected artifact beside its page rather than instead of it", async () => {
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

    // Both fields are set on purpose, and neither half is dropped for the other: the
    // page the run did produce is still this result, and the rejection still rides
    // above it. The notice is `role="alert"` so it is announced rather than sitting
    // there looking like another heading.
    expect(container.querySelectorAll(".design-preview-card")).toHaveLength(1);
    expect(container.querySelectorAll("iframe")).toHaveLength(1);
    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "The last artifact was rejected.",
    );
    expect(container.textContent).toContain("Artifact exceeds the size cap.");
    // The markup lives in the frame's attribute and nowhere else: the panel's own
    // document still renders none of it.
    expect(container.textContent).not.toContain("ARTIFACT MARKUP PAYLOAD");
  });

  it("carries no hardcoded string from the old mockup panel", async () => {
    await renderPanel();

    expect(container.textContent).not.toContain(MOCKUP_CARD_TITLE);
    expect(container.textContent).not.toContain("Mockup");
    expect(container.textContent).not.toContain("1 generation");
  });

  // The artifact branch renders the page and the agent's name for it and nothing else,
  // so there is no message text left in it to pair with anything: this pins that a
  // settled reply carrying no page never becomes a block, and that no reply commentary
  // reaches the panel. The store keeps ownership of which message an artifact belongs
  // to (see `latestArtifact` in src/store/appStore.ts).
  it("shows the preview alone in the artifact branch, never the transcript", async () => {
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

    expect(container.querySelectorAll("iframe")).toHaveLength(1);
    expect(container.querySelectorAll(".design-preview-card")).toHaveLength(1);
    expect(container.textContent).toContain("Made a card");
    expect(container.textContent).not.toContain("Built the card.");
    expect(container.textContent).not.toContain("src/card/source.tsx");
    expect(container.textContent).not.toContain("Tokens used");
    expect(container.textContent).not.toContain("Listed the tokens.");
    expect(container.textContent).not.toContain("src/tokens/source.ts");
  });

  // A run that produced no page takes the end of the transcript and so becomes the
  // store's latest artifact, but it does not take the finished results behind it with
  // it: the rejection is a notice above the stack, and every result still has a block.
  it("keeps earlier results when a later run's artifact is rejected", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { error: "Artifact exceeds the size cap." },
        // Store-reachable state: the injected latestArtifact matches the last message's
        // own artifact fields, as the store's predicate would produce it.
        messages: [
          {
            id: "assistant-1",
            role: "assistant",
            status: "done",
            title: "Landing page",
            desc: "",
            sources: [],
            nodeIds: [],
            artifactHtml: ARTIFACT_MARKUP,
          },
          {
            id: "assistant-2",
            role: "assistant",
            status: "done",
            title: "Pricing table",
            desc: "The artifact could not be displayed.",
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
    // The finished result is still here, and still reachable: a failed run takes the
    // end of the transcript, not the pages behind it.
    expect(container.querySelectorAll(".design-preview-card")).toHaveLength(1);
    expect(container.querySelectorAll(".design-preview-block-heading")).toHaveLength(1);
    expect(container.textContent).toContain("Landing page");
    // The failed run earns no block of its own: it has no page to show.
    expect(container.textContent).not.toContain("Pricing table");
    expect(container.textContent).not.toContain(NOTHING_GENERATED_MESSAGE);
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
    // No artifact exists, so no artifact frame may appear.
    expect(container.querySelectorAll("iframe")).toHaveLength(0);
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

  describe("preview geometry", () => {
    let restoreResizeObserver: () => void = () => undefined;

    beforeEach(() => {
      restoreResizeObserver = installResizeObserverStub();
    });

    afterEach(() => restoreResizeObserver());

    it("measures the scale from the column width and the page from the canvas store", async () => {
      useAppStore.setState(artifactSession(ARTIFACT_MARKUP));
      useAppStore.getState().setCanvasPageHeight(HOST, 922);
      await renderPanel();

      const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
      if (preview === null) throw new Error("preview element missing");
      // happy-dom has no layout, so the box the panel would measure is set here: 340 by
      // 600 is the sidebar at its default width with the header row and the card taking
      // the rest of a 740px column.
      Object.defineProperty(preview, "clientWidth", { value: 340, configurable: true });
      Object.defineProperty(preview, "clientHeight", { value: 600, configurable: true });

      const observer = ResizeObserverStub.instances.at(-1);
      if (observer === undefined) throw new Error("preview observer missing");
      expect(observer.observed).toHaveLength(1);
      expect(observer.observed[0]).toBe(preview);

      await act(async () => {
        observer.fire();
      });

      // 340 / 1280: the measured width against the width the page was authored at. The
      // constant 0.25 this replaced would read 320 at the default panel width and be wrong
      // at every other one the resize handle can reach.
      expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.265625");
      // The canvas viewport, not the column: 922px from the store paints as 245px, with
      // the rest of the column dark. The old column-height viewport read 2259px here.
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("922px");

      // The cleanup is what an unmount runs, not what a second artifact runs: the frame's
      // srcdoc changes and the box around it is the same node React already committed, so
      // the callback ref is not called again and the one observer keeps observing that same
      // element. Disconnecting and re-attaching here would buy nothing and open a frame
      // with no scale applied, on a box whose measurement cannot have changed.
      await act(async () => {
        useAppStore.setState(artifactSession("<p>second artifact</p>"));
      });
      expect(observer.disconnects).toBe(0);
      expect(ResizeObserverStub.instances).toHaveLength(1);
      expect(container.querySelector(".design-artifact-preview")).toBe(preview);
      expect(observer.observed).toEqual([preview]);
    });

    // The whole reason the ref applies the scale itself instead of letting the observer do
    // it: `observe()` delivers nothing until the next frame, and a frame at the stylesheet's
    // 0.25 is a frame of the wrong scale — what a new generation must never show. So this
    // test fires nothing: the box is measurable from the moment the node exists, and the two
    // properties are read back in the same commit that created them.
    it("applies the column's scale on commit, without waiting for a delivery", async () => {
      const restoreBox = stubBoxMetrics(340, 600);
      try {
        useAppStore.setState(artifactSession(ARTIFACT_MARKUP));
        await renderPanel();

        const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
        if (preview === null) throw new Error("preview element missing");
        const observer = ResizeObserverStub.instances.at(-1);
        if (observer === undefined) throw new Error("preview observer missing");
        // The observer is attached and has delivered nothing: the stub only fires by hand,
        // and nothing here fires it.
        expect(observer.observed).toEqual([preview]);

        expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.265625");
        // No canvas height was shared, so the canonical viewport stands in: the old
        // column-height viewport read 2259px at this box.
        expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
          `${ARTIFACT_PAGE_HEIGHT}px`,
        );
      } finally {
        restoreBox();
      }
    });

    // The shape the callback ref exists for: the box mounts while the html value stays put.
    // An artifact carrying both fields takes the error branch, so the box is absent until
    // the error clears — and an observer attached by an effect keyed on the html would never
    // run again, leaving the panel on the stylesheet's constants in silence.
    it("observes the box an artifact mounts while the html value is unchanged", async () => {
      const html = ARTIFACT_MARKUP;
      useAppStore.setState(
        session({ latestArtifact: { html, error: "Artifact exceeds the size cap." } }),
      );
      await renderPanel();

      // The error branch wins, so there is no box and nothing to observe yet.
      expect(container.querySelector(".design-artifact-preview")).toBeNull();
      expect(ResizeObserverStub.instances).toHaveLength(0);

      await act(async () => {
        useAppStore.setState(artifactSession(html));
      });

      const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
      if (preview === null) throw new Error("preview element missing");
      const observer = ResizeObserverStub.instances.at(-1);
      if (observer === undefined) throw new Error("preview observer missing");
      expect(observer.observed).toEqual([preview]);
    });

    it("leaves the stylesheet's values in place while the column has no size", async () => {
      useAppStore.setState(artifactSession(ARTIFACT_MARKUP));
      await renderPanel();

      const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
      if (preview === null) throw new Error("preview element missing");
      // An unmeasured box is 0x0 in this environment, and a collapsed sidebar reports the
      // same: the panel must not write a scale of 0 or a page of NaN over the defaults.
      const observer = ResizeObserverStub.instances.at(-1);
      if (observer === undefined) throw new Error("preview observer missing");
      await act(async () => {
        observer.fire();
      });

      expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe("");
      // The scale needs a box, but the page height comes from the canvas store:
      // an unmeasured box still paints the canonical viewport, never nothing.
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
        `${ARTIFACT_PAGE_HEIGHT}px`,
      );
    });

    // The height the panel writes belongs to the canvas, and the scale to the width:
    // a delivery that changes only the measured height moves neither, so the write is
    // guarded the way the canvas guards the same value. Two deliveries on one box, and
    // the second must reach the style object with nothing to say.
    it("writes nothing on a delivery that measures the box it already wrote", async () => {
      useAppStore.setState(artifactSession(ARTIFACT_MARKUP));
      await renderPanel();

      const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
      if (preview === null) throw new Error("preview element missing");
      Object.defineProperty(preview, "clientWidth", { value: 340, configurable: true });
      Object.defineProperty(preview, "clientHeight", { value: 600, configurable: true });
      const observer = ResizeObserverStub.instances.at(-1);
      if (observer === undefined) throw new Error("preview observer missing");
      await act(async () => {
        observer.fire();
      });

      // Same box, so the two custom properties already state what this delivery would derive.
      // Spying on the element's own style object is the honest measure: the property reads the
      // same value whether or not the write happened, so nothing else can tell them apart.
      const writes = vi.spyOn(preview.style, "setProperty");
      await act(async () => {
        observer.fire();
      });
      expect(writes).not.toHaveBeenCalled();
      writes.mockRestore();

      // …and the skip is an equality test, not a latch on the first measurement: a taller
      // box alone moves nothing now, because the page height belongs to the canvas store.
      // A box that did move in width is written, and only its scale.
      Object.defineProperty(preview, "clientHeight", { value: 620, configurable: true });
      const afterResize = vi.spyOn(preview.style, "setProperty");
      await act(async () => {
        observer.fire();
      });
      expect(afterResize).not.toHaveBeenCalled();
      afterResize.mockRestore();
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
        `${ARTIFACT_PAGE_HEIGHT}px`,
      );

      Object.defineProperty(preview, "clientWidth", { value: 360, configurable: true });
      await act(async () => {
        observer.fire();
      });
      expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe(
        String(360 / 1280),
      );
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
        `${ARTIFACT_PAGE_HEIGHT}px`,
      );
    });

    // A narrow box still scales from its width, and the page height still comes from
    // the store: the 18000px bound this replaced belonged to a viewport derived from
    // the box, which no longer exists. The stub below is deliberately narrower than the
    // narrowest preview the sidebar hands over (214px), and 3000 is taller than any
    // window — neither moves the page height now.
    it("scales a narrow box from its width while the page stays on the canvas height", async () => {
      useAppStore.setState(artifactSession(ARTIFACT_MARKUP));
      useAppStore.getState().setCanvasPageHeight(HOST, 922);
      await renderPanel();

      const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
      if (preview === null) throw new Error("preview element missing");
      Object.defineProperty(preview, "clientWidth", { value: 154, configurable: true });
      Object.defineProperty(preview, "clientHeight", { value: 3000, configurable: true });
      const observer = ResizeObserverStub.instances.at(-1);
      if (observer === undefined) throw new Error("preview observer missing");
      await act(async () => {
        observer.fire();
      });

      expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.1203125");
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("922px");
    });
  });

  // happy-dom does not lay out, so the half of the geometry that lives in the stylesheet —
  // that the page box reads its height from the property the panel writes, and that the
  // preview box declares no height of its own — can only be pinned by reading the file.
  describe("preview stylesheet", () => {
    it("takes the page box's height from the container, not from a constant", () => {
      const block = cssBlockFor(".design-artifact-preview-page {");
      expect(block).toContain("height: var(--design-artifact-preview-page-height)");
      expect(block).toContain("transform: scale(var(--design-artifact-preview-scale))");
      // The stylesheet cannot import the constant, so this is what keeps the page box at
      // the width the scale in DesignPreviewPanel.tsx is measured against.
      expect(block).toContain(`width: ${ARTIFACT_PAGE_WIDTH}px`);
      // The window over a longer page: the box clips to the viewport height.
      expect(block).toContain("overflow: hidden");
      expect(block).toContain("background: var(--white)");
    });

    it("leaves the rest of the column dark around the reduced page", () => {
      const block = cssBlockFor(".design-artifact-preview {");
      expect(block).toContain("background: var(--design-canvas)");
    });

    // The box a block claims is the page already scaled, and it is that height the block's
    // own basis is built from. The page box lays out at the 1280px the page was authored
    // at — the scale is a transform, which moves pixels and not the layout — so a box
    // without a stated height would size to a whole unscaled canvas page and a stack
    // would ask for one of those per result.
    it("sizes the preview to the scaled page, not to a whole one", () => {
      const block = cssBlockFor(".design-artifact-preview {");
      expect(block).toMatch(
        /height:\s*calc\(\s*var\(--design-artifact-preview-page-height\)\s*\*\s*var\(--design-artifact-preview-scale\)/,
      );
      expect(block).toContain("overflow: hidden");
    });

    // The chain that carries the sidebar's height down to the preview, pinned because nothing
    // else is: `min-height: 0` is what lets a flex item shrink below its own automatic minimum
    // size, so without it neither box can be squeezed into the column and the preview falls
    // back to its content — the defect this panel was built around. `height: 100%` is the
    // other half: a percentage height needs the parent's height to be definite.
    it("keeps the flex chain that carries the sidebar's height down to the preview", () => {
      const panel = cssBlockFor(".design-preview-panel {");
      expect(panel).toContain("display: flex");
      expect(panel).toContain("flex-direction: column");
      expect(panel).toContain("height: 100%");
      expect(panel).toContain("min-height: 0");

      const card = cssBlockFor(".design-preview-card {");
      expect(card).toContain("display: flex");
      expect(card).toContain("flex: 1 0 auto");
      expect(card).not.toContain("min-height");
    });

    // Every result stays reachable. A block that may shrink is handed a share too small
    // to hold its page, and the clipped band is fixed: the block's Shift+wheel window is
    // the canvas's window, not the block's, so a page that fits the canvas has no wheel
    // to scroll it with. Not shrinking overflows the stack instead, and the stack is the
    // scrollport — the panel above it and the sidebar around it stay where they are, so
    // the header row does not scroll away and the space below the last block is not a
    // scroll area of its own.
    it("keeps every result whole by scrolling the stack rather than shrinking its blocks", () => {
      const stack = cssBlockFor(".design-preview-stack {");
      expect(stack).toContain("overflow-y: auto");
      expect(stack).toMatch(/flex:\s*1/);
      expect(stack).toContain("min-height: 0");
      // The card's shrink factor is the load-bearing half: `flex: 1` alone would hand
      // every block a share of a fixed column however small that share got.
      expect(cssBlockFor(".design-preview-card {")).toContain("flex: 1 0 auto");
      // The box inside the block must not shrink either, or the card's basis is a whole
      // page again and the basis is what the stack overflows by.
      expect(cssBlockFor(".design-artifact-preview {")).toContain("flex: 1 0 auto");
      // …and neither box between the card and the page may carry a zero basis: the
      // card's own basis is built from what its children would take, so a `flex: 1`
      // on the opener would leave the card claiming no page and hand the stack
      // nothing to scroll by.
      expect(cssBlockFor(".design-artifact-preview-opener {")).toContain("flex: 1 0 auto");
    });

    // The reply card states text, and text does not shrink to fit. `.workspace-generation-card`
    // carries `overflow: hidden` (Workspace.css), and that same property zeroes the automatic
    // minimum size of a shrinkable flex item — so the reply card would compress and clip its
    // source chips instead of pushing them out to the sidebar, which is the surface that owns
    // this panel's scrolling. Nothing in happy-dom lays that out, so the rule that prevents it
    // is pinned by reading it.
    it("keeps the reply card at its content height", () => {
      const replyCard = cssBlockFor(
        ".design-preview-panel > .workspace-generation-card:not(.design-preview-card) {",
      );
      expect(replyCard).toMatch(/flex:\s*none/);
    });
  });
});

describe("preview shared viewport", () => {
  let restoreResizeObserver: () => void = () => undefined;
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    restoreResizeObserver = installResizeObserverStub();
    clearCachedArtifactSections();
    useAppStore.setState({ activeSurface: "workspace" });
    useAppStore.setState(session());
  });

  afterEach(async () => {
    restoreResizeObserver();
    clearCachedArtifactSections();
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

  async function renderWithBox(width: number, height: number): Promise<HTMLElement> {
    const restoreBox = stubBoxMetrics(width, height);
    try {
      await renderPanel();
      const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
      if (preview === null) throw new Error("preview element missing");
      return preview;
    } finally {
      restoreBox();
    }
  }

  function setArtifactWithCanvasHeight(html: string, canvasHeight: number | null): void {
    useAppStore.setState(artifactSession(html));
    if (canvasHeight !== null) {
      useAppStore.getState().setCanvasPageHeight(HOST, canvasHeight);
    }
  }

  it("uses the canvas viewport height at 340x600, not the panel height", async () => {
    setArtifactWithCanvasHeight(ARTIFACT_MARKUP, 922);
    const preview = await renderWithBox(340, 600);

    expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.265625");
    expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("922px");
  });

  it("falls back to the canonical height before the canvas is measured", async () => {
    setArtifactWithCanvasHeight(ARTIFACT_MARKUP, null);
    const preview = await renderWithBox(340, 600);

    expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.265625");
    expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
      `${ARTIFACT_PAGE_HEIGHT}px`,
    );
  });

  it("keeps the canvas height across narrow and wide resizes", async () => {
    setArtifactWithCanvasHeight(ARTIFACT_MARKUP, 922);
    useAppStore.setState(artifactSession(ARTIFACT_MARKUP));
    useAppStore.getState().setCanvasPageHeight(HOST, 922);
    await renderPanel();
    const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
    if (preview === null) throw new Error("preview element missing");
    const observer = ResizeObserverStub.instances.at(-1);
    if (observer === undefined) throw new Error("preview observer missing");

    Object.defineProperty(preview, "clientWidth", { value: 214, configurable: true });
    Object.defineProperty(preview, "clientHeight", { value: 600, configurable: true });
    await act(async () => {
      observer.fire();
    });
    expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe(
      String(214 / 1280),
    );
    expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("922px");

    Object.defineProperty(preview, "clientWidth", { value: 400, configurable: true });
    await act(async () => {
      observer.fire();
    });
    expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe(
      String(400 / 1280),
    );
    expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("922px");
  });

  it("frames height-dependent content at the canvas height so its layout matches", async () => {
    const heightDependent =
      "<style>@media (max-height: 1000px) { p { color: red; } }</style><p>viewport shaped</p>";
    setArtifactWithCanvasHeight(heightDependent, 922);
    const preview = await renderWithBox(340, 600);

    // 922 sits below the 1000px query the canvas sees; the old panel-height
    // viewport (2259px at this box) sat above it and laid the page out differently.
    expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("922px");
    expect(container.innerHTML).toContain("max-height: 1000px");
  });

  it("keeps a long page reachable in the shared window", async () => {
    const longHtml = `<main>${"long ".repeat(500)}</main>`;
    setCachedArtifactStructure(longHtml, { sections: [], contentHeight: 2641 });
    setArtifactWithCanvasHeight(longHtml, 922);
    const preview = await renderWithBox(340, 600);
    const frame = container.querySelector<HTMLIFrameElement>("iframe.design-artifact-frame");
    if (frame === null) throw new Error("preview frame missing");

    expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("922px");
    expect(frame.style.height).toBe("2641px");
    expect(frame.style.transform).toBe("translateY(0px)");
    const max = maxArtifactScroll(2641, 922);
    expect(max).toBe(1719);

    const card = container.querySelector<HTMLElement>(".design-preview-card");
    if (card === null) throw new Error("preview card missing");
    const wheel = new Event("wheel", { bubbles: true, cancelable: true }) as WheelEvent;
    Object.defineProperties(wheel, {
      deltaY: { value: 500 },
      deltaMode: { value: 0 },
      shiftKey: { value: true },
    });
    await act(async () => {
      card.dispatchEvent(wheel);
    });
    expect(frame.style.transform).toBe("translateY(-500px)");

    const far = new Event("wheel", { bubbles: true, cancelable: true }) as WheelEvent;
    Object.defineProperties(far, {
      deltaY: { value: 100000 },
      deltaMode: { value: 0 },
      shiftKey: { value: true },
    });
    await act(async () => {
      card.dispatchEvent(far);
    });
    expect(frame.style.transform).toBe(`translateY(${-max}px)`);
  });

  it("measures a cold-cache page so its tail is reachable without opening Design", async () => {
    // Normal delegated flow: the child finished in Workspace, the canvas never
    // mounted, so no store height and nothing in the critic cache.
    const html = `<main>${"cold ".repeat(200)}</main>`;
    useAppStore.setState(artifactSession(html));
    const restoreBox = stubBoxMetrics(340, 600);
    try {
      await renderPanel();
    } finally {
      restoreBox();
    }
    const frame = container.querySelector<HTMLIFrameElement>("iframe.design-artifact-frame");
    if (frame === null) throw new Error("preview frame missing");
    expect(container.querySelector(".design-artifact-preview-page")).not.toBeNull();
    expect(container.querySelector(".design-canvas-artifact-render-warning")).toBeNull();

    const measure = document.querySelector<HTMLIFrameElement>(".design-artifact-measurement-frame");
    if (measure === null || measure.contentWindow === null) {
      throw new Error("measurement frame did not mount on a cache miss");
    }
    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: criticResult(2641),
          source: measure.contentWindow,
        }),
      );
    });

    expect(getCachedArtifactStructure(html)?.contentHeight).toBe(2641);
    expect(frame.style.height).toBe("2641px");
    expect(frame.style.transform).toBe("translateY(0px)");
    // No verdict chrome rides along: the page and the dark remainder are the UI.
    expect(container.querySelector(".design-canvas-artifact-render-warning")).toBeNull();
    expect(container.textContent).not.toContain("Render checks");
    expect(container.textContent).not.toContain("too slow to check");

    const card = container.querySelector<HTMLElement>(".design-preview-card");
    if (card === null) throw new Error("preview card missing");
    const max = maxArtifactScroll(2641, ARTIFACT_PAGE_HEIGHT);
    await act(async () => {
      card.dispatchEvent(shiftWheel(100000));
    });
    expect(frame.style.transform).toBe(`translateY(${-max}px)`);
  });

  it("skips measurement on a cache hit", async () => {
    const html = `<main>${"warm ".repeat(200)}</main>`;
    setCachedArtifactStructure(html, { sections: [], contentHeight: 2641 });
    useAppStore.setState(artifactSession(html));
    const restoreBox = stubBoxMetrics(340, 600);
    try {
      await renderPanel();
    } finally {
      restoreBox();
    }

    expect(document.querySelector(".design-artifact-measurement-frame")).toBeNull();
    const frame = container.querySelector<HTMLIFrameElement>("iframe.design-artifact-frame");
    expect(frame?.style.height).toBe("2641px");
  });

  it("ignores a late result for a replaced artifact", async () => {
    const first = "<main>first page</main>";
    const second = "<main>second page</main>";
    useAppStore.setState(artifactSession(first));
    const restoreBox = stubBoxMetrics(340, 600);
    try {
      await renderPanel();
    } finally {
      restoreBox();
    }
    const firstMeasure = document.querySelector<HTMLIFrameElement>(
      ".design-artifact-measurement-frame",
    );
    if (firstMeasure === null || firstMeasure.contentWindow === null) {
      throw new Error("measurement frame did not mount for the first artifact");
    }
    const staleWindow = firstMeasure.contentWindow;

    await act(async () => {
      useAppStore.setState(artifactSession(second));
    });
    const secondMeasure = document.querySelector<HTMLIFrameElement>(
      ".design-artifact-measurement-frame",
    );
    if (secondMeasure === null || secondMeasure.contentWindow === null) {
      throw new Error("measurement frame did not mount for the second artifact");
    }
    expect(firstMeasure.isConnected).toBe(false);

    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", { data: criticResult(3000), source: staleWindow }),
      );
    });
    expect(getCachedArtifactStructure(first)).toBeUndefined();

    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: criticResult(2500),
          source: secondMeasure.contentWindow,
        }),
      );
    });
    expect(getCachedArtifactStructure(second)?.contentHeight).toBe(2500);
    const frame = container.querySelector<HTMLIFrameElement>("iframe.design-artifact-frame");
    expect(frame?.style.height).toBe("2500px");
  });

  it("carries the measurement frame rules in the shared stylesheet, not the lazy one", () => {
    expect(PREVIEW_CSS).toContain(".design-artifact-measurement-frame");
    const block = cssBlockFor(".design-artifact-measurement-frame");
    expect(block).toContain("position: fixed");
    expect(block).toContain("left: -100000px");
    expect(block).toContain("visibility: hidden");
    expect(DESIGN_CSS).not.toContain(".design-artifact-measurement-frame");
  });
});

// The Live preview opened out to the window: the same page the column shows,
// in the transcript's own lightbox frame, fitted to the box the window leaves.
describe("preview enlarged viewer", () => {
  /** A stage box chosen so each fit lands on an exact number. */
  const STAGE = { width: 1200, height: 700 };
  const CANVAS_HEIGHT = 1000;

  let restoreResizeObserver: () => void = () => undefined;
  let container: HTMLDivElement;
  let root: Root;
  let overlayRoot: Root | null = null;
  let overlayHost: HTMLDivElement | null = null;

  beforeEach(() => {
    restoreResizeObserver = installResizeObserverStub();
    clearCachedArtifactSections();
    useAppStore.setState({ activeSurface: "workspace" });
    useAppStore.setState(session());
  });

  afterEach(async () => {
    restoreResizeObserver();
    clearCachedArtifactSections();
    await act(async () => overlayRoot?.unmount());
    overlayHost?.remove();
    overlayRoot = null;
    overlayHost = null;
    await act(async () => root.unmount());
    container?.remove();
    document.body.style.overflow = "";
    useAppStore.setState(session());
  });

  async function renderPanel(html = ARTIFACT_MARKUP, canvasHeight: number | null = CANVAS_HEIGHT) {
    container = document.createElement("div");
    document.body.appendChild(container);
    useAppStore.setState(artifactSession(html));
    if (canvasHeight !== null) {
      useAppStore.getState().setCanvasPageHeight(HOST, canvasHeight);
    }
    root = createRoot(container);
    await act(async () => {
      root.render(<DesignPreviewPanel />);
    });
  }

  /**
   * Presses the opener while the stage has a box: the viewer fits its own box in
   * a callback ref, so the box has to exist when it mounts.
   */
  async function openViewer(html = ARTIFACT_MARKUP, canvasHeight: number | null = CANVAS_HEIGHT) {
    await renderPanel(html, canvasHeight);
    const opener = container.querySelector<HTMLButtonElement>(".design-artifact-preview-opener");
    if (opener === null) throw new Error("preview opener missing");
    const restoreBox = stubBoxMetrics(STAGE.width, STAGE.height);
    try {
      await act(async () => {
        opener.click();
      });
    } finally {
      restoreBox();
    }
    const stage = container.querySelector<HTMLElement>(".design-artifact-viewer-page");
    if (stage === null) throw new Error("viewer stage missing");
    return { opener, stage };
  }

  function dialog(): HTMLElement {
    const found = container.querySelector<HTMLElement>('[role="dialog"]');
    if (found === null) throw new Error("viewer dialog missing");
    return found;
  }

  it("opens an enlarged dialog from the preview's own button", async () => {
    await renderPanel(ARTIFACT_MARKUP);

    const opener = container.querySelector<HTMLButtonElement>(".design-artifact-preview-opener");
    if (opener === null) throw new Error("preview opener missing");
    // A real button is what makes Enter and Space open it; the name is what tells
    // a screen reader what pressing it does.
    expect(opener.tagName).toBe("BUTTON");
    expect(opener.getAttribute("type")).toBe("button");
    expect(opener.getAttribute("aria-label")).toBe("Enlarge Design preview");
    // The block's own three targets and nothing else: Open Design, the heading that
    // opens the canvas on this result, and the opener. Nothing nests a button inside
    // one, and the artifact itself is not focusable.
    expect(container.querySelectorAll("button")).toHaveLength(3);
    expect(container.querySelectorAll("button button")).toHaveLength(0);

    const restoreBox = stubBoxMetrics(STAGE.width, STAGE.height);
    try {
      await act(async () => {
        opener.click();
      });
    } finally {
      restoreBox();
    }

    expect(container.querySelector('[role="dialog"]')?.getAttribute("aria-modal")).toBe("true");
    expect(container.querySelector('[role="dialog"]')?.getAttribute("aria-label")).toBe(
      "Design preview",
    );
  });

  it("shows the same page, fitted to the window, under the artifact policy", async () => {
    const { stage } = await openViewer();

    // 700/1000 binds before 1200/1280, so the page is reduced to the height it
    // has to fit and the width it had is left over as dark margin.
    expect(stage.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.7");
    // The canvas page height, the very one the column paints: same page, bigger box.
    expect(stage.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("1000px");
    const column = container.querySelector<HTMLElement>(".design-artifact-preview");
    expect(column?.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("1000px");

    const frame = dialog().querySelector<HTMLIFrameElement>("iframe");
    expect(frame?.getAttribute("sandbox")).toBe("");
    expect(frame?.getAttribute("scrolling")).toBe("no");
    expect(frame?.style.pointerEvents).toBe("none");
    const srcDoc = frame?.getAttribute("srcdoc") ?? "";
    expect(srcDoc.startsWith(`${ARTIFACT_CSP_META}\n`)).toBe(true);
    expect(srcDoc).toContain(ARTIFACT_MARKUP);
    // `inert` sits on the page box, not the stage: the generated page is out of
    // the focus order, and the stage keeps taking the wheel the window is on.
    expect(frame?.closest(".design-artifact-preview-page")?.hasAttribute("inert")).toBe(true);
    expect(stage.hasAttribute("inert")).toBe(false);
  });

  it("lets the width bind instead, when the window is tall enough", async () => {
    await renderPanel();
    const opener = container.querySelector<HTMLButtonElement>(".design-artifact-preview-opener");
    if (opener === null) throw new Error("preview opener missing");
    const restoreBox = stubBoxMetrics(900, 1000);
    try {
      await act(async () => {
        opener.click();
      });
    } finally {
      restoreBox();
    }

    const stage = container.querySelector<HTMLElement>(".design-artifact-viewer-page");
    expect(stage?.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.703125");
  });

  it("refits when the window is reshaped", async () => {
    const { stage } = await openViewer();
    const observer = ResizeObserverStub.instances.at(-1);
    if (observer === undefined) throw new Error("viewer observer missing");
    expect(observer.observed).toEqual([stage]);

    Object.defineProperty(stage, "clientWidth", { value: 800, configurable: true });
    Object.defineProperty(stage, "clientHeight", { value: 1000, configurable: true });
    await act(async () => {
      observer.fire();
    });

    expect(stage.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.625");
  });

  it("leaves the stage's values in place while the dialog has no box", async () => {
    await renderPanel();
    const opener = container.querySelector<HTMLButtonElement>(".design-artifact-preview-opener");
    if (opener === null) throw new Error("preview opener missing");
    // No box metrics at all: the same 0x0 box a collapsed sidebar hands over.
    await act(async () => {
      opener.click();
    });

    const stage = container.querySelector<HTMLElement>(".design-artifact-viewer-page");
    if (stage === null) throw new Error("viewer stage missing");
    expect(stage.style.getPropertyValue("--design-artifact-preview-scale")).toBe("");
    // The page height needs no box, so it lands even here.
    expect(stage.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("1000px");
  });

  it("returns focus to the opener when Escape closes it", async () => {
    const { opener } = await openViewer();

    expect(dialog().contains(document.activeElement)).toBe(true);
    await act(async () => {
      press("Escape");
    });

    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  it("closes from its own close control and from the scrim", async () => {
    const { opener } = await openViewer();

    const close = container.querySelector<HTMLButtonElement>(".chat-image-lightbox-close");
    if (close === null) throw new Error("close control missing");
    expect(close.getAttribute("aria-label")).toBe("Close Design preview");
    await act(async () => {
      close.click();
    });
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(opener);

    await act(async () => {
      opener.click();
    });
    const scrim = container.querySelector<HTMLElement>(".chat-image-lightbox-scrim");
    if (scrim === null) throw new Error("scrim missing");
    await act(async () => {
      scrim.dispatchEvent(
        new MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0 }),
      );
    });
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(opener);
  });

  // The frame the transcript's viewer is built on is shared, so the two are one
  // stack: the image viewer opened over this one is the one Escape reaches, and
  // neither takes the other's Tab trap or scroll lock with it when it closes.
  it("keeps Tab inside and closes only the topmost of two viewers", async () => {
    const { opener } = await openViewer();

    const design = dialog();
    const closeControl = design.querySelector<HTMLElement>(".chat-image-lightbox-close");
    if (closeControl === null) throw new Error("close control missing");
    closeControl.focus();
    await act(async () => {
      press("Tab");
    });
    expect(design.contains(document.activeElement)).toBe(true);

    overlayHost = document.createElement("div");
    document.body.appendChild(overlayHost);
    const overlay = createRoot(overlayHost);
    overlayRoot = overlay;
    await act(async () => {
      overlay.render(<TransientImageLightbox />);
    });
    expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(2);

    await act(async () => {
      press("Escape");
    });
    expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(1);
    expect(container.querySelector('[role="dialog"]')).not.toBeNull();

    await act(async () => {
      press("Escape");
    });
    expect(document.querySelectorAll('[role="dialog"]')).toHaveLength(0);
    expect(document.activeElement).toBe(opener);
  });

  it("scrolls a cold-cache page to its tail once the critic has measured it", async () => {
    // Nothing in the cache and no canvas height shared: the delegated cold start,
    // with the enlarged window already open over the column.
    const html = `<main>${"cold ".repeat(200)}</main>`;
    const { stage } = await openViewer(html, null);
    expect(dialog().querySelector<HTMLIFrameElement>("iframe")?.style.height).toBe("");

    const measure = document.querySelector<HTMLIFrameElement>(".design-artifact-measurement-frame");
    if (measure === null || measure.contentWindow === null) {
      throw new Error("measurement frame did not mount on a cache miss");
    }
    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: criticResult(2641),
          source: measure.contentWindow,
        }),
      );
    });

    const column = container.querySelector<HTMLIFrameElement>(".design-artifact-preview iframe");
    const enlarged = dialog().querySelector<HTMLIFrameElement>("iframe");
    expect(column?.style.height).toBe("2641px");
    expect(enlarged?.style.height).toBe("2641px");
    expect(enlarged?.style.transform).toBe("translateY(0px)");

    // The same Shift+wheel window the column has, clamped the same way.
    const max = maxArtifactScroll(2641, ARTIFACT_PAGE_HEIGHT);
    await act(async () => {
      stage.dispatchEvent(shiftWheel(100000));
    });
    expect(enlarged?.style.transform).toBe(`translateY(${-max}px)`);
  });

  it("closes on a replaced artifact, letting the observer go with it", async () => {
    const html = `<main>${"long ".repeat(500)}</main>`;
    setCachedArtifactStructure(html, { sections: [], contentHeight: 2641 });
    const { stage } = await openViewer(html);
    const observer = ResizeObserverStub.instances.at(-1);
    if (observer === undefined) throw new Error("viewer observer missing");

    await act(async () => {
      useAppStore.setState(artifactSession("<main>second page</main>"));
    });

    // A window over an artifact the canvas no longer holds would be showing
    // something that is gone.
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(observer.disconnects).toBe(1);
    expect(stage.isConnected).toBe(false);
  });

  it("closes when the host is dropped", async () => {
    await openViewer();

    await act(async () => {
      useAppStore.setState(session({ host: null }));
    });

    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(container.textContent).toContain(EMPTY_CANVAS_MESSAGE);
  });

  it("leaves the dark remainder around the fitted page", () => {
    // happy-dom does not lay out, so the stage's ground and the centring of the
    // page inside it are only checkable by reading the sheet.
    expect(cssBlockFor(".design-artifact-viewer-page {")).toContain(
      "background: var(--design-canvas)",
    );
    const centred = cssBlockFor(".design-artifact-viewer-page .design-artifact-preview-page {");
    expect(centred).toContain("left: 50%");
    expect(centred).toContain("top: 50%");
    // The scale hangs the page from the layout box's own top-left corner, so
    // centring it means taking half the rendered size off in both axes. A
    // positive half would drop the page a whole height below the stage.
    expect(centred).toMatch(/margin-left: calc\([^;]*-\s*640px/);
    expect(centred).toMatch(/margin-top: calc\([^;]*(\/ -2|\* -1 \/ 2)/);
    // The stage is what fills the card, so the card's own scroll never runs. The
    // rule is paired with the lightbox card's own class: alone it would be the
    // same weight as that rule and lose on whichever sheet the build emits last.
    expect(cssBlockFor(".chat-image-lightbox.design-artifact-viewer {")).toContain(
      "align-items: stretch",
    );
  });

  it("gives the enlarged card a size, so the stage has a box to fit into", () => {
    // The shell caps the card with `max-*` and centres it as a grid item, which
    // leaves an auto-sized card wrapping whatever is inside it: the bar. The
    // page box is absolute and can never give that card a height, so the size
    // has to be stated here, at the shell's own bounds.
    const card = cssBlockFor(".chat-image-lightbox.design-artifact-viewer {");
    expect(card).toContain("width: calc(100vw - 48px)");
    expect(card).toContain("height: calc(100vh - 48px)");
  });

  it("keeps the preview's column intact under the opener", () => {
    // The opener stands where the preview box used to stand in the card, so it
    // has to hand that flex item on rather than becoming a block the page's own
    // height sizes.
    const opener = cssBlockFor(".design-artifact-preview-opener {");
    expect(opener).toContain("display: flex");
    expect(opener).toContain("flex-direction: column");
    expect(opener).toMatch(/flex:\s*1/);
    expect(opener).toContain("min-height: 0");
    expect(opener).toContain("align-items: stretch");
  });
});

// The column stacks one block per finished result, in transcript order, so a
// transcript that holds two Design agents' results shows both pages instead of
// only the one that finished last.
describe("stacked preview blocks", () => {
  const PAGE_A = '<main data-probe="a">PAGE A</main>';
  const PAGE_B = '<main data-probe="b">PAGE B</main>';
  const CHILD_A = "child-a";
  const CHILD_B = "child-b";

  let container: HTMLDivElement;
  let root: Root;

  /** The card the mirror writes for a finished child (delegatedDesignMirror.ts). */
  function mirroredCard(childSessionId: string, title: string, html: string): DesignMessage {
    return {
      id: delegatedDesignCardId(childSessionId),
      role: "assistant",
      status: "done",
      title,
      desc: "Finished by a Design agent.",
      sources: [],
      nodeIds: [],
      artifactHtml: html,
    } as DesignMessage;
  }

  function twoResults(): { designSession: DesignSessionState } {
    return session({
      latestArtifact: { html: PAGE_B },
      messages: [
        mirroredCard(CHILD_A, "Landing page", PAGE_A),
        mirroredCard(CHILD_B, "Pricing table", PAGE_B),
      ],
    });
  }

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

  function blocks(): HTMLElement[] {
    return [...container.querySelectorAll<HTMLElement>(".design-preview-card")];
  }

  function headings(): HTMLButtonElement[] {
    return [...container.querySelectorAll<HTMLButtonElement>(".design-preview-block-heading")];
  }

  it("gives every finished result its own titled block, in transcript order", async () => {
    useAppStore.setState(twoResults());

    await renderPanel();

    expect(blocks()).toHaveLength(2);
    // Named by the agent that made each one: two pages in one column are only
    // tellable apart by what is under them.
    expect(headings().map((heading) => heading.textContent)).toEqual([
      "Landing page",
      "Pricing table",
    ]);
    // Transcript order is the column's order, block for block.
    const pages = [...container.querySelectorAll<HTMLIFrameElement>("iframe")].map(
      (frame) => frame.getAttribute("srcdoc") ?? "",
    );
    expect(pages).toHaveLength(2);
    expect(pages[0]).toContain(PAGE_A);
    expect(pages[1]).toContain(PAGE_B);
  });

  // Two runs answering to the same name — the mirror writes the child's display name
  // verbatim, and the host answers a file-writing run with a plain "Wrote" — would
  // otherwise leave two byte-identical headings, two buttons with the same accessible
  // name and two sections labelled the same, tellable only by recognising the pixels
  // below. Two runs that named nothing would collide the same way.
  it("names every block, and never the same name twice", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { html: PAGE_B },
        messages: [
          mirroredCard(CHILD_A, "Landing page", PAGE_A),
          mirroredCard("child-c", "Landing page", PAGE_A),
          mirroredCard("child-d", "Wrote", PAGE_A),
          mirroredCard("child-e", "", PAGE_A),
          mirroredCard("child-f", "   ", PAGE_A),
        ],
      }),
    );

    await renderPanel();

    const labels = headings().map((heading) => heading.textContent);
    expect(labels).toEqual(["Landing page 1", "Landing page 2", "Wrote", "Result 1", "Result 2"]);
    // A name that only one block answers to keeps its own spelling, unnumbered.
    expect(labels).toContain("Wrote");
    expect(labels.every((label) => label !== null && label.trim() !== "")).toBe(true);
    expect(new Set(labels).size).toBe(labels.length);
    // The card is named for the same block, so the region and its heading agree.
    expect(blocks().map((block) => block.getAttribute("aria-label"))).toEqual(labels);
  });

  it("gives a message with no page behind it no block", async () => {
    useAppStore.setState(
      session({
        latestArtifact: { html: PAGE_A },
        messages: [
          mirroredCard(CHILD_A, "Landing page", PAGE_A),
          {
            id: "assistant-2",
            role: "assistant",
            status: "done",
            title: "Tokens used",
            desc: "Listed the tokens.",
            sources: ["src/tokens/source.ts"],
            nodeIds: [],
          },
        ],
      }),
    );

    await renderPanel();

    expect(blocks()).toHaveLength(1);
    expect(container.querySelectorAll("iframe")).toHaveLength(1);
    // The reply card does not come back either: one result is enough to fill the
    // column, and a reply that made no page is commentary about a run.
    expect(container.textContent).not.toContain("Tokens used");
    expect(container.textContent).not.toContain(NOTHING_GENERATED_MESSAGE);
  });

  it("renders a single result as the one block it was before the stack", async () => {
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
            artifactHtml: ARTIFACT_MARKUP,
          },
        ],
      }),
    );

    await renderPanel();

    expect(container.querySelector(".design-preview-stack")).not.toBeNull();
    expect(blocks()).toHaveLength(1);
    const frame = container.querySelector<HTMLIFrameElement>("iframe");
    if (frame === null) throw new Error("preview frame missing");
    // The same page render the single card carried: same policy, same confinement,
    // same scaled page box, and still no reply commentary on the card.
    expect(frame.getAttribute("sandbox")).toBe("");
    expect(frame.getAttribute("scrolling")).toBe("no");
    expect(frame.style.pointerEvents).toBe("none");
    expect(frame.closest(".design-artifact-preview")?.hasAttribute("inert")).toBe(true);
    expect(frame.parentElement?.className).toContain("design-artifact-preview-page");
    expect(container.textContent).not.toContain("Snapped the radius to the token.");
    expect(container.textContent).not.toContain("src/real/source.tsx");
    // The markup lives in the frame's attribute and nowhere else, so the panel's own
    // document still renders none of it.
    expect(container.textContent).not.toContain("ARTIFACT MARKUP PAYLOAD");
  });

  it("keeps the empty and absent-host sentences exactly as they were", async () => {
    useAppStore.setState({
      designSession: {
        host: null,
        document: null,
        messages: [],
        latestArtifact: null,
        generation: null,
        sectionNotes: [],
        lastCanvasPageHeight: null,
        mirroredFitPending: false,
      },
    });
    await renderPanel();
    expect(container.querySelector(".workspace-design-note")?.textContent).toBe(
      EMPTY_CANVAS_MESSAGE,
    );
    expect(container.querySelectorAll(".design-preview-card")).toHaveLength(0);

    await act(async () => root.unmount());
    container.remove();
    useAppStore.setState(session());
    await renderPanel();
    expect(container.querySelector(".workspace-design-note")?.textContent).toBe(
      NOTHING_GENERATED_MESSAGE,
    );
    expect(container.querySelectorAll(".design-preview-card")).toHaveLength(0);
  });

  it("opens the canvas on the mirrored child whose heading was clicked", async () => {
    useAppStore.setState(twoResults());

    await renderPanel();
    const [first] = headings();
    if (first === undefined) throw new Error("block heading missing");
    await act(async () => {
      first.click();
    });

    const open = useAppStore.getState().designSession;
    // The canvas renders the last card holding an artifact, so opening the first
    // block moves that child's card to the end and leaves the other where it was.
    expect(open.messages.map((message) => message.id)).toEqual([
      delegatedDesignCardId(CHILD_B),
      delegatedDesignCardId(CHILD_A),
    ]);
    expect(open.latestArtifact).toMatchObject({ html: PAGE_A });
    expect(open.mirroredFitPending).toBe(true);
    expect(useAppStore.getState().activeSurface).toBe("design");
  });

  it("gives the stack a share of the column each and only a gap between them", () => {
    // happy-dom does not lay out, so the stack's share and the gap between blocks
    // are only checkable by reading the sheet.
    const stack = cssBlockFor(".design-preview-stack {");
    expect(stack).toContain("display: flex");
    expect(stack).toContain("flex-direction: column");
    expect(stack).toMatch(/flex:\s*1/);
    expect(stack).toContain("min-height: 0");
    // A gap, not a border and not a panel: the dark remainder inside each block
    // already reads as the edge of a page.
    expect(stack).toContain("gap: var(--space-8)");
    expect(stack).not.toContain("border");
    expect(stack).not.toContain("background");

    const heading = cssBlockFor(".design-preview-block-heading {");
    expect(heading).toContain("border: 0");
    expect(heading).toContain("background: none");
    expect(heading).not.toContain("border-radius");
  });
});
