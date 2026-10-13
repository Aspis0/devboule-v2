import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { artifactSrcDoc } from "./artifactCsp";
import {
  ARTIFACT_PAGE_WIDTH,
  clampArtifactScroll,
  maxArtifactScroll,
  scrollArtifactBy,
} from "./artifactViewport";
import { getCachedArtifactStructure, setCachedArtifactStructure } from "./artifactStructure";
import { ArtifactRenderCritic, type ArtifactRenderCriticResult } from "./artifactRenderCritic";
import { DesignPreviewLightbox } from "./DesignPreviewLightbox";
import { openDesignResult } from "./designResultOpen";
import type { DesignAssistantMessage, DesignMessage } from "./designHost";

/** A finished assistant message that carries a page the column can preview. */
export type DesignPreviewBlockMessage = DesignAssistantMessage & { artifactHtml: string };

/**
 * Whether one transcript message earns a block in the column. The mirror writes its
 * cards and the surface writes its replies as done assistant messages, and a message
 * with no page behind it has nothing to show, so it gets no block.
 */
export function previewsArtifact(message: DesignMessage): message is DesignPreviewBlockMessage {
  return (
    message.role === "assistant" && message.status === "done" && message.artifactHtml !== undefined
  );
}

/**
 * One finished result in the sidebar's Live preview column: the agent's name over the
 * page it produced, reduced to the column the same way the canvas reduces it. Each
 * block owns its own scale, measurement and scroll window because each shows a
 * different page, and the enlarged window opens on the block it was opened from.
 */
export function DesignPreviewBlock({
  message,
  canvasPageHeight,
}: {
  message: DesignPreviewBlockMessage;
  /** The canvas viewport height: the page is shown at this height, scaled to the column. */
  canvasPageHeight: number;
}) {
  const html = message.artifactHtml;
  // Two results in one column have to be told apart by their agent, so the block is
  // named; a reply that recorded no name is still named by what it is.
  const heading = message.title !== "" ? message.title : "Design result";

  // The preview is the one part of a block that is a function of the sidebar's size:
  // the page is authored at ARTIFACT_PAGE_WIDTH and the transform has to bring it down
  // to whatever width the column actually has, so the scale cannot be a constant. One
  // observer, one custom property for the scale, no state — a resize must not re-render
  // the block or rescan its page. The page height comes from the canvas store, never
  // from this box, so measuring it here cannot feed back in.
  // A callback ref, not an effect keyed on the html: the observer's lifetime is the node's
  // lifetime. It attaches when the node mounts and detaches when it unmounts, so no
  // rendering choice upstream can leave a mounted preview without one.
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
  }, [canvasPageHeight, html]);

  // A page the canvas never measured has no cache entry yet; the critic below
  // primes it once, and this state applies its height the moment it lands.
  const [measuredPreview, setMeasuredPreview] = useState<{
    html: string;
    contentHeight?: number;
  } | null>(null);
  const handleMeasured = useCallback((html: string, result: ArtifactRenderCriticResult) => {
    setCachedArtifactStructure(html, {
      sections: result.structure ?? [],
      ...(result.contentHeight === undefined ? {} : { contentHeight: result.contentHeight }),
    });
    setMeasuredPreview({ html, contentHeight: result.contentHeight });
  }, []);
  const cacheHeight = getCachedArtifactStructure(html)?.contentHeight;
  const contentHeight =
    measuredPreview !== null && measuredPreview.html === html
      ? measuredPreview.contentHeight
      : cacheHeight;
  const needsMeasure = contentHeight === undefined;
  const [previewScroll, setPreviewScroll] = useState(0);
  const previewScrollRef = useRef(0);
  useEffect(() => {
    previewScrollRef.current = 0;
    setPreviewScroll(0);
  }, [html]);
  const scrollOffset =
    contentHeight === undefined
      ? 0
      : clampArtifactScroll(previewScroll, contentHeight, canvasPageHeight);

  useEffect(() => {
    // The card, not the preview box: `inert` takes a subtree out of hit testing, so a
    // listener on the inert box would never fire.
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
  }, [canvasPageHeight, contentHeight, html]);

  // The enlarged page is the page, not a picture of it: it is open against this
  // block's artifact, so a replacement page or an unmounted block closes it rather
  // than leaving a window showing something the canvas dropped.
  const [enlarged, setEnlarged] = useState<{ html: string; opener: HTMLElement } | null>(null);
  const viewer = enlarged !== null && enlarged.html === html ? enlarged : null;

  return (
    <Fragment>
      <section
        className="workspace-generation-card design-preview-card"
        aria-label={heading}
        ref={cardRef}
      >
        {/*
          The heading is the block's name and its open-on-canvas affordance at once: the
          canvas shows the last result that holds an artifact, so opening one result
          from a column that stacks several means moving that result to the end first.
        */}
        <button
          type="button"
          className="design-preview-block-heading"
          onClick={() => openDesignResult(message)}
        >
          {heading}
        </button>
        {/*
          The preview is the open target, so the page is inside a button —
          the transcript's own thumbnail contract — and the opener is what
          focus returns to. Only the box is the button's child: the frame and
          its page keep the measurement and the policy they already had.

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
        <button
          type="button"
          className="design-artifact-preview-opener"
          aria-label="Enlarge Design preview"
          onClick={(event) => setEnlarged({ html, opener: event.currentTarget })}
        >
          <div className="design-artifact-preview" inert ref={attachPreview}>
            <div className="design-artifact-preview-page">
              <iframe
                sandbox=""
                srcDoc={artifactSrcDoc(html)}
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
        </button>
        {needsMeasure ? (
          <ArtifactRenderCritic html={html} onResult={handleMeasured} measureOnly />
        ) : null}
      </section>
      {/* Outside the card's own frame, so nothing about the card clips the scrim. */}
      {viewer === null ? null : (
        <DesignPreviewLightbox
          html={viewer.html}
          canvasPageHeight={canvasPageHeight}
          contentHeight={contentHeight}
          opener={viewer.opener}
          onClose={() => setEnlarged(null)}
        />
      )}
    </Fragment>
  );
}
