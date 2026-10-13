import { useEffect, useState } from "react";

/**
 * Whether `element` has come within a screen of the viewport. The answer
 * latches: once near it stays near, so a picture read on its first approach
 * is not dropped when it scrolls away. True where no observer exists.
 */
export function useNearViewport(element: HTMLElement | null): boolean {
  const [near, setNear] = useState(() => typeof IntersectionObserver === "undefined");
  useEffect(() => {
    if (near || element === null || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[entries.length - 1]?.isIntersecting) {
          setNear(true);
          observer.disconnect();
        }
      },
      { rootMargin: "100% 0px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [element, near]);
  return near;
}
