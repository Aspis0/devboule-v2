/**
 * A line count as the sidebar prints it beside a row: exact under a thousand,
 * one decimal below ten thousand (`1.1k`), whole thousands above (`14k`). The
 * exact figure rides the row's tooltip, so the short form loses nothing a
 * person needs at a glance.
 */
export function compactLineCount(count: number): string {
  if (count < 1000) return String(count);
  if (count < 10_000) {
    const tenths = Math.round(count / 100) / 10;
    return `${Number.isInteger(tenths) ? tenths : tenths.toFixed(1)}k`;
  }
  return `${Math.round(count / 1000)}k`;
}
