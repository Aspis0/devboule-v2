import { useEffect, useRef, useState, type ReactNode } from "react";

/** A table's scroll wrapper that becomes a focusable, named region only
 * while the table actually overflows it — below that it stays a plain
 * div, so a long transcript grows no keyboard stop per table. The name
 * comes from the header cells. happy-dom computes no layout, so the
 * overflow itself is only provable in the live app. */
export function TableScrollRegion({
  headers,
  children,
}: {
  headers: string[];
  children: ReactNode;
}) {
  const wrapperRef = useRef<HTMLDivElement>(null);
  const [overflows, setOverflows] = useState(false);
  useEffect(() => {
    const wrapper = wrapperRef.current;
    const table = wrapper !== null ? wrapper.firstElementChild : null;
    if (wrapper === null || table === null) return;
    const measure = () => setOverflows(table.scrollWidth > wrapper.clientWidth);
    const observer = new ResizeObserver(measure);
    observer.observe(wrapper);
    observer.observe(table);
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
