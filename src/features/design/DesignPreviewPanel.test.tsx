// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAppStore, type DesignSessionState } from "../../store/appStore";
import type { DesignHost } from "./designHost";
import { ARTIFACT_CSP_META } from "./artifactCsp";
import { ARTIFACT_PAGE_WIDTH } from "./artifactViewport";
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

// The preview's stylesheet, read as text. happy-dom does not lay out, so the half of the
// geometry that lives in CSS — which property the page box reads its height from, and
// that the preview box declares no height of its own — is only checkable here.
// designPills.test.ts reads design.css the same way.
const PREVIEW_CSS = readFileSync(
  join(dirname(fileURLToPath(import.meta.url)), "artifactPreview.css"),
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
    // The page and nothing else. This branch used to carry the artifact message's title,
    // description and source chips; the card is now the frame around the preview, so none
    // of that text may reach it.
    expect(container.querySelector(".design-preview-card")).not.toBeNull();
    expect(container.textContent).not.toContain("Edited the card");
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
    expect(container.textContent).not.toContain(NOT_OPENED_MESSAGE);
    expect(container.textContent).not.toContain(REJECTED_MESSAGE);
  });

  it("renders no preview frame when the artifact carries an error", async () => {
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

    // Both fields are set on purpose: the error branch wins, so a rejected artifact
    // is never rendered as if it had passed.
    expect(container.querySelectorAll("iframe")).toHaveLength(0);
    // The stronger half of the original prohibition, kept where it still holds. The
    // positive case had to give it up — the markup now lives in the frame's `srcdoc`
    // attribute, so "nowhere in the DOM" stopped being expressible there. On this
    // branch nothing renders the artifact at all, so the whole-DOM assertion is still
    // the honest one: a rejected artifact must not reach the document by any route,
    // including an attribute nobody looked at.
    expect(container.innerHTML).not.toContain("ARTIFACT MARKUP PAYLOAD");
  });

  it("carries no hardcoded string from the old mockup panel", async () => {
    await renderPanel();

    expect(container.textContent).not.toContain(MOCKUP_CARD_TITLE);
    expect(container.textContent).not.toContain("Mockup");
    expect(container.textContent).not.toContain("1 generation");
  });

  // The artifact branch renders the page and nothing else, so there is no message text
  // left in it to pair with anything: this pins that neither the artifact's own message
  // nor the later settled reply reaches the panel. The store keeps ownership of which
  // message an artifact belongs to (see `latestArtifact` in src/store/appStore.ts), and
  // the panel no longer re-derives that predicate — with no text on the card, a wrong
  // pairing has nothing to show itself in.
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
    expect(container.textContent).not.toContain("Made a card");
    expect(container.textContent).not.toContain("Built the card.");
    expect(container.textContent).not.toContain("src/card/source.tsx");
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
    // Rejected means not previewed either: the error branch renders no frame.
    expect(container.querySelectorAll("iframe")).toHaveLength(0);
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

    it("measures the page from the column the observer reports, not from a constant", async () => {
      useAppStore.setState(session({ latestArtifact: { html: ARTIFACT_MARKUP } }));
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
      // 600 / 0.265625 = 2258.8… document pixels, which the transform paints as 600. A
      // page-height constant would be 800px here and 200px painted — the shape the
      // committente saw as a third-full column.
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
        "2259px",
      );

      // The cleanup is what an unmount runs, not what a second artifact runs: the frame's
      // srcdoc changes and the box around it is the same node React already committed, so
      // the callback ref is not called again and the one observer keeps observing that same
      // element. Disconnecting and re-attaching here would buy nothing and open a frame
      // with no scale applied, on a box whose measurement cannot have changed.
      await act(async () => {
        useAppStore.setState(session({ latestArtifact: { html: "<p>second artifact</p>" } }));
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
        useAppStore.setState(session({ latestArtifact: { html: ARTIFACT_MARKUP } }));
        await renderPanel();

        const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
        if (preview === null) throw new Error("preview element missing");
        const observer = ResizeObserverStub.instances.at(-1);
        if (observer === undefined) throw new Error("preview observer missing");
        // The observer is attached and has delivered nothing: the stub only fires by hand,
        // and nothing here fires it.
        expect(observer.observed).toEqual([preview]);

        expect(preview.style.getPropertyValue("--design-artifact-preview-scale")).toBe("0.265625");
        expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
          "2259px",
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
        useAppStore.setState(session({ latestArtifact: { html } }));
      });

      const preview = container.querySelector<HTMLElement>(".design-artifact-preview");
      if (preview === null) throw new Error("preview element missing");
      const observer = ResizeObserverStub.instances.at(-1);
      if (observer === undefined) throw new Error("preview observer missing");
      expect(observer.observed).toEqual([preview]);
    });

    it("leaves the stylesheet's values in place while the column has no size", async () => {
      useAppStore.setState(session({ latestArtifact: { html: ARTIFACT_MARKUP } }));
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
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe("");
    });

    // The property the panel writes is the page box's height, and the page box is inside the
    // element being measured: the CSS gives that element a definite height today, so the value
    // cannot come back as a delivery — but nothing in the panel enforces that, so the write is
    // guarded the way the canvas guards the same value. Two deliveries on one box, and the
    // second must reach the style object with nothing to say.
    it("writes nothing on a delivery that measures the box it already wrote", async () => {
      useAppStore.setState(session({ latestArtifact: { html: ARTIFACT_MARKUP } }));
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

      // …and the skip is an equality test, not a latch on the first measurement: a box that
      // did move is written. 620 / 0.265625 = 2334.1 document pixels.
      Object.defineProperty(preview, "clientHeight", { value: 620, configurable: true });
      const afterResize = vi.spyOn(preview.style, "setProperty");
      await act(async () => {
        observer.fire();
      });
      expect(afterResize).toHaveBeenCalledWith("--design-artifact-preview-page-height", "2334px");
      afterResize.mockRestore();
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
        "2334px",
      );
    });

    // The bound that keeps a broken chain to one wrong frame. 154 is the narrowest preview the
    // sidebar can hand this panel (MIN_PANEL_WIDTH 180, less the sidebar's 24px of gutters and
    // the card's 2px of border) and 3000 is taller than any window, so 3000 / 0.1203125 =
    // 24935px of document is a page no legitimate layout asks for: the bound is what the page
    // box gets, and the value stays one delivery away from itself instead of compounding.
    it("bounds the page height it asks a document for", async () => {
      useAppStore.setState(session({ latestArtifact: { html: ARTIFACT_MARKUP } }));
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
      expect(preview.style.getPropertyValue("--design-artifact-preview-page-height")).toBe(
        "18000px",
      );
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
    });

    it("gives the preview the column's height instead of one page", () => {
      const block = cssBlockFor(".design-artifact-preview {");
      expect(block).toMatch(/flex:\s*1/);
      // No height of its own: a `height:` here would be the fixed box this replaced.
      expect(block).not.toMatch(/^\s+height:/m);
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
      expect(card).toMatch(/flex:\s*1/);
      expect(card).toContain("min-height: 0");
    });

    // The reply card states text, and text does not shrink to fit. `.workspace-generation-card`
    // carries `overflow: hidden` (Workspace.css) so that the preview card can be squeezed into
    // the column, and that same property zeroes the automatic minimum size of a shrinkable flex
    // item — so the reply card would compress and clip its source chips instead of pushing them
    // out to the sidebar, which is the surface that owns this panel's scrolling. Nothing in
    // happy-dom lays that out, so the rule that prevents it is pinned by reading it.
    it("keeps the reply card at its content height without catching the preview card", () => {
      const replyCard = cssBlockFor(
        ".design-preview-panel > .workspace-generation-card:not(.design-preview-card) {",
      );
      expect(replyCard).toMatch(/flex:\s*none/);
      // The `:not()` is the load-bearing half, not decoration: unscoped, this selector is two
      // classes and a pseudo-class and would outrank `.design-preview-card`, so the preview card
      // would be `flex: none` — one page tall in a column it has to fill — whichever order the
      // two rules are written in.
      expect(cssBlockFor(".design-preview-card {")).toMatch(/flex:\s*1/);
    });
  });
});
