import { useEffect, useRef, useState, type ReactNode } from "react";

/** A plain div until a real overflow, so a long transcript grows no keyboard stop per table.
 * Happy-dom computes no layout, so the overflow itself stays a live-app check. */
export function TableScrollRegion({
  headers,
  children,
}: {
  /** Plain rendered `<th>` text: up to three non-blank values form the overflow region's accessible name. */
  headers: string[];
  children: ReactNode;
}) {
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [overflows, setOverflows] = useState(false);
  useEffect(() => {
    const wrapper = wrapperRef.current;
    const child = wrapper !== null ? wrapper.firstElementChild : null;
    if (wrapper === null || child === null) return;
    if (typeof ResizeObserver === "undefined") return;
    const measure = () => setOverflows(child.scrollWidth > wrapper.clientWidth);
    const observer = new ResizeObserver(measure);
    observer.observe(wrapper);
    observer.observe(child);
    return () => observer.disconnect();
  }, []);
  const named = headers.filter((cell) => cell.trim() !== "").slice(0, 3);
  const label = named.length > 0 ? `Table: ${named.join(", ")}` : "Table";
  return (
    <div
      ref={wrapperRef}
      className="plan-markdown-table-scroll"
      {...(overflows ? { role: "region" as const, "aria-label": label, tabIndex: 0 } : {})}
    >
      {children}
    </div>
  );
}
