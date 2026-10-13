import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useAppStore } from "../../store/appStore";
import { artifactSrcDoc } from "./artifactCsp";
import {
  ARTIFACT_PAGE_HEIGHT,
  ARTIFACT_PAGE_WIDTH,
  clampArtifactScroll,
  maxArtifactScroll,
  scrollArtifactBy,
} from "./artifactViewport";
import { getCachedArtifactStructure, setCachedArtifactStructure } from "./artifactStructure";
import { ArtifactRenderCritic, type ArtifactRenderCriticResult } from "./artifactRenderCritic";
// The panel renders `design-artifact-frame` and the preview rules beside it, and those
// live in their own stylesheet so importing them here cannot drag the 58 KB Design
// stylesheet into the Workspace chunk, which loads on every start.
import "./artifactPreview.css";

// The preview shows the canvas page reduced: the same viewport height the canvas
// measures, scaled to the column width, with the rest of the column dark.

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
  const canvasPageHeight = useAppStore(
    (state) => state.designSession.lastCanvasPageHeight ?? ARTIFACT_PAGE_HEIGHT,
  );
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
  // observer, one custom property for the scale, no state — a resize must not re-render
  // the panel or rescan the transcript. The page height comes from the canvas store,
  // never from this box, so measuring it here cannot feed back in.
  // A callback ref, not an effect keyed on the html: the observer's lifetime is the node's
  // lifetime. It attaches when the node mounts and detaches when it unmounts, whichever
  // branch rendered it, so no rendering choice in another branch can leave a mounted
  // preview without one. An effect keyed on a value only coincides with that lifetime for
  // as long as the value being set happens to imply the node being there.
  const previewRef = useRef<HTMLDivElement | null>(null);
  const cardRef = useRef<HTMLDivElement | null>(null);
  const attachPreview = useCallback((element: HTMLDivElement) => {
    previewRef.current = element;
    const apply = () => {
      const width = element.clientWidth;
      // A collapsed or not-yet-laid-out panel has no box to scale into: the stylesheet's
      // defaults stay until it does, and the observer fires again once it has a size.
      if (width <= 0) return;
      const scale = width / ARTIFACT_PAGE_WIDTH;
      // The read is of the inline style only, so a stylesheet default can never
      // suppress the first write.
      if (element.style.getPropertyValue("--design-artifact-preview-scale") !== String(scale)) {
        element.style.setProperty("--design-artifact-preview-scale", String(scale));
      }
    };
    // Apply once before observing: the observer's first delivery is a frame away, and a
    // frame of the wrong scale is exactly what a new generation would show.
    apply();
    const observer = new ResizeObserver(apply);
    observer.observe(element);
    return () => {
      previewRef.current = null;
      observer.disconnect();
    };
  }, []);

  useEffect(() => {
    const element = previewRef.current;
    if (!element) return;
    const value = `${canvasPageHeight}px`;
    if (element.style.getPropertyValue("--design-artifact-preview-page-height") !== value) {
      element.style.setProperty("--design-artifact-preview-page-height", value);
    }
  }, [canvasPageHeight, latestArtifact?.html]);

  const artifactHtml = latestArtifact?.html;
  // A page the canvas never measured has no cache entry yet; the critic below
  // primes it once, and this state applies its height the moment it lands.
  const [measuredPreview, setMeasuredPreview] = useState<{
    html: string;
    contentHeight?: number;
  } | null>(null);
  const handlePreviewMeasured = useCallback((html: string, result: ArtifactRenderCriticResult) => {
    setCachedArtifactStructure(html, {
      sections: result.structure ?? [],
      ...(result.contentHeight === undefined ? {} : { contentHeight: result.contentHeight }),
    });
    setMeasuredPreview({ html, contentHeight: result.contentHeight });
  }, []);
  const cacheHeight =
    artifactHtml === undefined
      ? undefined
      : getCachedArtifactStructure(artifactHtml)?.contentHeight;
  const contentHeight =
    measuredPreview !== null && measuredPreview.html === artifactHtml
      ? measuredPreview.contentHeight
      : cacheHeight;
  const needsMeasure = artifactHtml !== undefined && contentHeight === undefined;
  const [previewScroll, setPreviewScroll] = useState(0);
  const previewScrollRef = useRef(0);
  useEffect(() => {
    previewScrollRef.current = 0;
    setPreviewScroll(0);
  }, [artifactHtml]);
  const scrollOffset =
    contentHeight === undefined
      ? 0
      : clampArtifactScroll(previewScroll, contentHeight, canvasPageHeight);

  useEffect(() => {
    const card = cardRef.current;
    if (!card || contentHeight === undefined) return;
    if (maxArtifactScroll(contentHeight, canvasPageHeight) <= 0) return;
    const handleWheel = (event: WheelEvent) => {
      if (!event.shiftKey) return;
      const next = scrollArtifactBy(
        previewScrollRef.current,
        { deltaY: event.deltaY, deltaMode: event.deltaMode },
        contentHeight,
        canvasPageHeight,
      );
      if (next === previewScrollRef.current) return;
      event.preventDefault();
      previewScrollRef.current = next;
      setPreviewScroll(next);
    };
    card.addEventListener("wheel", handleWheel, { passive: false });
    return () => card.removeEventListener("wheel", handleWheel);
  }, [canvasPageHeight, contentHeight, artifactHtml]);

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
          Nothing on the canvas yet. A Design agent's result appears here when it finishes.
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
            <div className="workspace-generation-card design-preview-card" ref={cardRef}>
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

                `scrolling="no"` is the third: the frame is a window over the page, and
                without it a document taller than the window grows a scrollbar inside
                the thumbnail — a control the user cannot use, drawn over ~15px of the
                layout width the page was authored at.
              */}
              <div className="design-artifact-preview" inert ref={attachPreview}>
                <div className="design-artifact-preview-page">
                  <iframe
                    sandbox=""
                    srcDoc={artifactSrcDoc(latestArtifact.html)}
                    title="Generated artifact preview"
                    className="design-artifact-frame"
                    scrolling="no"
                    style={
                      contentHeight === undefined
                        ? { pointerEvents: "none" }
                        : {
                            pointerEvents: "none",
                            height: `${Math.max(canvasPageHeight, contentHeight)}px`,
                            transform: `translateY(${-scrollOffset}px)`,
                          }
                    }
                  />
                </div>
              </div>
              {needsMeasure && artifactHtml !== undefined ? (
                <ArtifactRenderCritic
                  html={artifactHtml}
                  onResult={handlePreviewMeasured}
                  measureOnly
                />
              ) : null}
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
