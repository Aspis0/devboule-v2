import { useCallback, useEffect, useState, type RefObject } from "react";

/** Which sides of the strip still hide chips: the fade renders only there. */
export interface StripFade {
  left: boolean;
  right: boolean;
}

/** Tracks the scrollport's hidden sides so the 36 px fade renders only
 * where chips are still hidden. Re-reads on scroll and on size changes. */
export function useStripFade(scrollportRef: RefObject<HTMLDivElement | null>): StripFade {
  const [fade, setFade] = useState<StripFade>({ left: false, right: false });

  const read = useCallback(() => {
    const port = scrollportRef.current;
    if (port === null) return;
    const max = port.scrollWidth - port.clientWidth;
    setFade({
      left: port.scrollLeft > 0,
      // A chip exactly filling the port leaves no hidden side: the 1 px
      // slack keeps a rounding edge from painting a fade to nothing.
      right: port.scrollLeft < max - 1,
    });
  }, [scrollportRef]);

  useEffect(() => {
    read();
    const port = scrollportRef.current;
    if (port === null) return;
    port.addEventListener("scroll", read, { passive: true });
    const observer = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(read);
    observer?.observe(port);
    return () => {
      port.removeEventListener("scroll", read);
      observer?.disconnect();
    };
  }, [read, scrollportRef]);

  return fade;
}
