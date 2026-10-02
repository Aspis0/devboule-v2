export function formatCount(value: number): string {
  return value.toLocaleString("en-US").replaceAll(",", " ");
}

/**
 * Costs truncate at every magnitude — the copy never shows more than the
 * provider billed. The cut is made on the shortest decimal string that
 * round-trips to the number (`String`), which carries no rounding step at
 * all: toFixed at any guard depth either rounds up across the cut
 * (`0.9999999`) or, deep enough to stop that, exposes the binary expansion
 * and truncates a cent low (`0.0055`). Below a tenth of a cent a truncated
 * figure would read as zero, so it shows as `<$0.0001`; zero and anything
 * not a number (a NaN or an infinity can never mean a bill) show nothing at
 * all. Whole dollars past 1e21: string form switches to an exponent there,
 * and slicing exponent text prints a fragment.
 */
export function usdCopy(cost: number): string | null {
  if (!Number.isFinite(cost) || cost <= 0) return null;
  if (cost >= 1e21) return `$${Math.floor(cost).toLocaleString("en-US")}`;
  if (cost < 0.0001) return "<$0.0001";
  const decimals = cost < 0.01 ? 4 : 2;
  const text = String(cost);
  const point = text.indexOf(".");
  const cut =
    point < 0
      ? `${text}.${"0".repeat(decimals)}`
      : text.slice(0, point + 1 + decimals).padEnd(point + 1 + decimals, "0");
  return `$${Number(cut).toLocaleString("en-US", {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  })}`;
}
