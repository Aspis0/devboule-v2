import { useCallback, useEffect, useRef, useState, type RefObject } from "react";

/** Which sides of the strip still hide chips: the fade renders only there. */
export interface StripFade {
  left: boolean;
  right: boolean;
}

/** Tracks the scrollport's hidden sides so the 36 px fade renders only
 * where chips are still hidden. Scroll bursts coalesce to one read per
 * animation frame, an unchanged answer keeps its state object so the strip
 * does not re-render, and the roster is an input: adding or closing chips
 * changes the scroll width without firing any scroll or resize event. */
export function useStripFade(
  scrollportRef: RefObject<HTMLDivElement | null>,
  tabs: readonly { id: string }[],
): StripFade {
  const [fade, setFade] = useState<StripFade>({ left: false, right: false });

  const read = useCallback(() => {
    const port = scrollportRef.current;
    if (port === null) return;
    const max = port.scrollWidth - port.clientWidth;
    const next = {
      left: port.scrollLeft > 0,
      // A chip exactly filling the port leaves no hidden side: the 1 px
      // slack keeps a rounding edge from painting a fade to nothing.
      right: port.scrollLeft < max - 1,
    };
    setFade((prev) => (prev.left === next.left && prev.right === next.right ? prev : next));
  }, [scrollportRef]);

  const frame = useRef<number | null>(null);
  const schedule = useCallback(() => {
    if (frame.current !== null) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = null;
      read();
    });
  }, [read]);

  useEffect(() => {
    read();
    const port = scrollportRef.current;
    if (port === null) return;
    port.addEventListener("scroll", schedule, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    observer?.observe(port);
    return () => {
      port.removeEventListener("scroll", schedule);
      observer?.disconnect();
      if (frame.current !== null) {
        cancelAnimationFrame(frame.current);
        frame.current = null;
      }
    };
  }, [read, schedule, scrollportRef]);

  useEffect(() => {
    read();
  }, [read, tabs]);

  return fade;
}
