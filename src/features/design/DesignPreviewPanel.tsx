import { useCallback, useMemo } from "react";
import { useAppStore } from "../../store/appStore";
import { artifactSrcDoc } from "./artifactCsp";
import { ARTIFACT_PAGE_WIDTH } from "./artifactViewport";
// The panel renders `design-artifact-frame` and the preview rules beside it, and those
// live in their own stylesheet so importing them here cannot drag the 58 KB Design
// stylesheet into the Workspace chunk, which loads on every start. Relying on
// `DesignSurface` having been mounted instead would lean on an invariant that is true
// today and will not stay true: the store is not persisted, so `latestArtifact` can only
// come from a generation that went through the canvas — until a commissioned result
// arrives from another session, at which point a panel styled by a stylesheet nobody
// loaded would render a 1280px page into a 340px column. Vite serves one copy however
// many modules ask for it.
import "./artifactPreview.css";

// The tallest page the panel will ask a document for, in document pixels. The ask is
// `columnHeight / scale`, the scale is `columnWidth / ARTIFACT_PAGE_WIDTH`, so the ask is
// `columnHeight * ARTIFACT_PAGE_WIDTH / columnWidth` — and both factors are bounded by the
// sidebar's own box (workspaceResize.ts), not by anything inside the page:
//
//   columnWidth  >= MIN_PANEL_WIDTH (180) less the sidebar's 12px gutters
//                   (.workspace-side-scroll) and the card's 1px border
//                   (.workspace-generation-card) = 154
//   columnHeight <= 2160, the content height of a 4K window at 100% scaling, the tallest
//                   viewport this panel is plausibly run in — the column is never taller
//                   than the window it scrolls in
//
// 2160 * 1280 / 154 = 17953.25, so 18000 sits above every layout the sidebar can produce
// and cannot clamp one. It is there for the layout the sidebar cannot produce: the value is
// derived from a measurement and written back as an inline property on the element that was
// measured, and a child consumes it, so a CSS chain that stopped giving that element a
// definite height would feed the value back in as a measurement and grow it on every
// delivery instead of failing once. The canvas clamps the same class of problem to
// [ARTIFACT_PAGE_MIN_HEIGHT, ARTIFACT_PAGE_MAX_HEIGHT] (artifactViewport.ts); that clamp is
// wrong here, because this box is deliberately taller than the canvas's and 2000px would
// crop every document longer than one canvas page — the defect this panel removed.
const ARTIFACT_PREVIEW_MAX_PAGE_HEIGHT = 18000;

// Absence meanings in this panel:
// - host === null: Design was never opened in this app session. The document is not
//   persisted and the surface unmounts when deselected, so there is nothing to mirror —
//   this is not "empty work" and not an error.
// - latestArtifact === null: no finished artifact exists (or only unfinished turns); it is
//   the store's own definition of "what the canvas renders", so the panel never recomputes it.
// - generation !== null: a run is writing into an assistant message right now.
export function DesignPreviewPanel() {
  // Field-by-field selectors, not one designSession selector: this panel stays mounted
  // while a generation on the Design surface keeps writing to the store, and a whole-object
  // subscription would re-render on every one of those writes.
  const host = useAppStore((state) => state.designSession.host);
  const generation = useAppStore((state) => state.designSession.generation);
  const latestArtifact = useAppStore((state) => state.designSession.latestArtifact);
  const messages = useAppStore((state) => state.designSession.messages);
  const selectSurface = useAppStore((state) => state.selectSurface);

  // The last settled reply, and the only transcript this panel still reads: the branch
  // that has an artifact renders the preview alone, so no message text is consulted
  // there. Memoised on `messages` so unrelated store writes do not rescan. The lib is
  // ES2022, so Array.findLast is unavailable; the loop is the copy-free equivalent.
  const lastSettled = useMemo(() => {
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (message?.role === "assistant" && message.status === "done") {
        return message;
      }
    }
    return undefined;
  }, [messages]);

  // Absence of sources on a card's message means the run reported none; rendering no
  // chips is the honest presentation, never a guessed path.
  const sources = lastSettled?.sources ?? [];
  const desc = lastSettled?.desc ?? "";

  // The preview is the one part of this panel that is a function of the sidebar's size:
  // the page is authored at ARTIFACT_PAGE_WIDTH and the transform has to bring it down
  // to whatever width the column actually has, so the scale cannot be a constant. One
  // observer, two custom properties, no state — a resize must not re-render the panel or
  // rescan the transcript.
  // A callback ref, not an effect keyed on the html: the observer's lifetime is the node's
  // lifetime. It attaches when the node mounts and detaches when it unmounts, whichever
  // branch rendered it, so no rendering choice in another branch can leave a mounted
  // preview without one. An effect keyed on a value only coincides with that lifetime for
  // as long as the value being set happens to imply the node being there.
  const attachPreview = useCallback((element: HTMLDivElement) => {
    const apply = () => {
      const width = element.clientWidth;
      const height = element.clientHeight;
      // A collapsed or not-yet-laid-out panel has no box to scale into: the stylesheet's
      // defaults stay until it does, and the observer fires again once it has a size.
      if (width <= 0 || height <= 0) return;
      const scale = width / ARTIFACT_PAGE_WIDTH;
      // The page box is measured in document pixels and the transform scales them back
      // down, so the column's height becomes `height / scale` of document. The document's
      // own height is not a factor and is never consulted — see artifactPreview.css.
      // Bounded by ARTIFACT_PREVIEW_MAX_PAGE_HEIGHT, for the reason written there: this is
      // an inline style on the element being measured, so an unbroken chain is the only
      // thing keeping the value from re-entering as a measurement.
      const pageHeight = Math.min(Math.round(height / scale), ARTIFACT_PREVIEW_MAX_PAGE_HEIGHT);
      // An equal value is not written again — the canvas's own idiom for the same property
      // (`setArtifactPageHeight((current) => (current === desired ? current : desired))` in
      // DesignSurface.tsx). The read is of the inline style only, so a stylesheet default
      // can never suppress the first write, and a box that changed writes both properties.
      const write = (property: string, value: string) => {
        if (element.style.getPropertyValue(property) !== value) {
          element.style.setProperty(property, value);
        }
      };
      write("--design-artifact-preview-scale", String(scale));
      write("--design-artifact-preview-page-height", `${pageHeight}px`);
    };
    // Apply once before observing: the observer's first delivery is a frame away, and a
    // frame of the wrong scale is exactly what a new generation would show.
    apply();
    const observer = new ResizeObserver(apply);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  return (
    <div className="design-preview-panel">
      <div className="workspace-grounding-row">
        <span>Live preview</span>
        <button
          type="button"
          className="workspace-open-design"
          onClick={() => selectSurface("design")}
        >
          Open Design
        </button>
      </div>
      {host === null ? (
        <p className="workspace-design-note">
          Design has not been opened in this session yet, so there is nothing to mirror here.
        </p>
      ) : (
        <>
          {generation !== null ? (
            <div className="workspace-design-note" role="status">
              Generating a design…
            </div>
          ) : null}
          {latestArtifact !== null && latestArtifact.error !== undefined ? (
            <div className="workspace-design-note" role="alert">
              <div>The last artifact was rejected.</div>
              <div>{latestArtifact.error}</div>
            </div>
          ) : latestArtifact !== null && latestArtifact.html !== undefined ? (
            // The preview and nothing else. A Design run writes no files — the artifact is
            // HTML inside the reply, scraped by extractFencedHtml — so the title, the
            // description and the source chips this card used to carry were commentary
            // about a run that never produced the paths they named. The canvas is where the
            // artifact's own context belongs; the sidebar shows the page.
            <div className="workspace-generation-card design-preview-card">
              {/*
                The panel renders the artifact itself, not a description of it: same
                sandbox and the same `artifactSrcDoc`, so the preview cannot render
                under a weaker policy than the canvas.

                `inert` is here for the same reason the canvas carries it
                (`DesignSurface.tsx`, `design-canvas-artifact-content`), and it is not
                the same guarantee as `pointerEvents: none`: that one stops the mouse,
                this one takes the frame out of the focus order. A generated page may
                contain links and fields, and without `inert` a Tab from the panel
                walks into model-written markup that nothing here meant to be reachable.

                `scrolling="no"` is the third: the page lays out in a box that is taller
                or shorter than the document, and without it a document taller than the
                box grows a scrollbar inside the thumbnail — a control the user cannot
                use, drawn over ~15px of the layout width the page was authored at.
              */}
              <div className="design-artifact-preview" inert ref={attachPreview}>
                <div className="design-artifact-preview-page">
                  <iframe
                    sandbox=""
                    srcDoc={artifactSrcDoc(latestArtifact.html)}
                    title="Generated artifact preview"
                    className="design-artifact-frame"
                    scrolling="no"
                    style={{ pointerEvents: "none" }}
                  />
                </div>
              </div>
            </div>
          ) : lastSettled !== undefined ? (
            <div className="workspace-generation-card">
              <div className="workspace-generation-heading">
                <span>{lastSettled.title !== "" ? lastSettled.title : "Latest reply"}</span>
              </div>
              {desc !== "" ? <div className="workspace-design-desc">{desc}</div> : null}
              {sources.length > 0 ? (
                <div className="workspace-design-sources">
                  {sources.map((source, index) => (
                    <span key={`${source}-${index}`}>{source}</span>
                  ))}
                </div>
              ) : null}
            </div>
          ) : generation === null ? (
            // While a first generation runs, "nothing has been generated yet"
            // would contradict the banner above it.
            <p className="workspace-design-note">
              Design is open, but nothing has been generated yet.
            </p>
          ) : null}
        </>
      )}
    </div>
  );
}
