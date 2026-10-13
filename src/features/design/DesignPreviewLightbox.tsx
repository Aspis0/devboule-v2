import { useCallback, useEffect, useRef, useState } from "react";
import { LightboxDialog } from "../../components/LightboxDialog";
import { artifactSrcDoc } from "./artifactCsp";
import {
  ARTIFACT_PAGE_WIDTH,
  clampArtifactScroll,
  maxArtifactScroll,
  scrollArtifactBy,
} from "./artifactViewport";

/**
 * The sidebar's Live preview opened out to the window. It is the same page the
 * column shows — same sandbox, same `artifactSrcDoc`, same canvas page height,
 * same dark remainder and the same Shift+wheel window over a longer one — drawn
 * through a bigger box, fitted to whatever width and height the window has left.
 */
export function DesignPreviewLightbox({
  html,
  canvasPageHeight,
  contentHeight,
  opener,
  onClose,
}: {
  html: string;
  /** The canvas viewport height: the page is shown at this height, as in the column. */
  canvasPageHeight: number;
  /** The measured page height; undefined until the render critic has measured it. */
  contentHeight: number | undefined;
  /** The preview button that opened this; focus returns to it on close. */
  opener: HTMLElement | null;
  onClose: () => void;
}) {
  // The box decides the scale, so it is measured and never assumed — and measured
  // from a callback ref for the panel's reason: the observer's first delivery is
  // a frame away, and a frame at the stylesheet's default scale is a frame of
  // the wrong size.
  const stageRef = useRef<HTMLDivElement | null>(null);
  const attachStage = useCallback(
    (element: HTMLDivElement) => {
      stageRef.current = element;
      const apply = () => {
        element.style.setProperty("--design-artifact-preview-page-height", `${canvasPageHeight}px`);
        const width = element.clientWidth;
        const height = element.clientHeight;
        // A dialog with no box yet has nothing to fit into: the stylesheet's
        // values stand until it does, and the observer fires again once it has a size.
        if (width <= 0 || height <= 0) return;
        // Fitted to both edges, so one uniform reduction holds whichever binds:
        // a wide window leaves dark gutters down the sides, a short one letterboxes.
        const scale = Math.min(width / ARTIFACT_PAGE_WIDTH, height / canvasPageHeight);
        if (element.style.getPropertyValue("--design-artifact-preview-scale") !== String(scale)) {
          element.style.setProperty("--design-artifact-preview-scale", String(scale));
        }
      };
      apply();
      const observer = new ResizeObserver(apply);
      observer.observe(element);
      return () => {
        stageRef.current = null;
        observer.disconnect();
      };
    },
    [canvasPageHeight],
  );

  const [scroll, setScroll] = useState(0);
  const scrollRef = useRef(0);
  useEffect(() => {
    scrollRef.current = 0;
    setScroll(0);
  }, [html]);
  const scrollOffset =
    contentHeight === undefined ? 0 : clampArtifactScroll(scroll, contentHeight, canvasPageHeight);

  // The wheel listener rides the stage, not the page box: `inert` takes a
  // subtree out of hit testing, so a listener on the inert box would never fire.
  useEffect(() => {
    const stage = stageRef.current;
    if (stage === null || contentHeight === undefined) return;
    if (maxArtifactScroll(contentHeight, canvasPageHeight) <= 0) return;
    const handleWheel = (event: WheelEvent) => {
      if (!event.shiftKey) return;
      const next = scrollArtifactBy(
        scrollRef.current,
        { deltaY: event.deltaY, deltaMode: event.deltaMode },
        contentHeight,
        canvasPageHeight,
      );
      if (next === scrollRef.current) return;
      event.preventDefault();
      scrollRef.current = next;
      setScroll(next);
    };
    stage.addEventListener("wheel", handleWheel, { passive: false });
    return () => stage.removeEventListener("wheel", handleWheel);
  }, [canvasPageHeight, contentHeight, html]);

  return (
    <LightboxDialog
      label="Design preview"
      closeLabel="Close Design preview"
      opener={opener}
      onClose={onClose}
      className="design-artifact-viewer"
    >
      {/*
        The same three invariants the column's preview carries: the frame is the
        canvas frame, not a second renderer; `inert` takes the generated page out
        of the focus order; `scrolling="no"` keeps it a window rather than a box
        with a scrollbar the user cannot use.
      */}
      <div className="design-artifact-viewer-page" ref={attachStage}>
        <div className="design-artifact-preview-page" inert>
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
    </LightboxDialog>
  );
}
