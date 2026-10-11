export type ChoiceStep = "previous" | "next" | "first" | "last";

/**
 * The option a key moves to. Disabled options are skipped and the walk wraps,
 * as native radios do. Null when every option is disabled.
 */
export function stepChoiceIndex(
  disabled: readonly boolean[],
  from: number,
  step: ChoiceStep,
): number | null {
  const count = disabled.length;
  if (step === "first") {
    const first = disabled.indexOf(false);
    return first === -1 ? null : first;
  }
  if (step === "last") {
    const last = disabled.lastIndexOf(false);
    return last === -1 ? null : last;
  }
  const delta = step === "next" ? 1 : -1;
  for (let offset = 1; offset <= count; offset += 1) {
    const candidate = (((from + delta * offset) % count) + count) % count;
    if (!disabled[candidate]) return candidate;
  }
  return null;
}
