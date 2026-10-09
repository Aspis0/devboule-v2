// Every outline or ring-shadow that reads the accent directly — not through
// `--ring` — is registered with the ground it is drawn on (focusRingGrounds.ts),
// and that ground must follow the theme and hold 3:1 against the accent in both
// themes. A new focus rule that reads the accent on an unregistered ground, or
// on a ground that stays dark in both themes, fails here: this is how a ring on
// a tool row's image thumbnails would have been found.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { contrastRatio } from "./contrast";
import { DIRECT_RING_GROUNDS } from "./focusRingGrounds";
import { hex, label, RULES, THEMES, type SheetRule } from "./sheetRules";

const DARK_IN_BOTH_THEMES = new Set(["--code-bg", "--code-control"]);
const RING_FLOOR = 3;
const DIRECT = /var\(\s*--(accent|terracotta)(-deep|-pressed)?\s*\)/;
const RING_PAINT = new Set(["outline", "outline-color", "box-shadow"]);

/** Whether a rule draws a focus outline, or a ring-shadow, in the accent itself
 * (`--accent-soft` tints never match the exact var). */
function readsAccentDirectly(rule: SheetRule): boolean {
  return [...rule.declarations].some(
    ([property, value]) => RING_PAINT.has(property) && DIRECT.test(value),
  );
}

const directRules = RULES.filter(readsAccentDirectly);

function entriesFor(rule: SheetRule) {
  const first = rule.selector.split(",")[0]!.trim();
  return DIRECT_RING_GROUNDS.filter((entry) => entry.file === rule.file && entry.match === first);
}

describe("focus rings that read the accent directly", () => {
  it("found them", () => {
    expect(directRules.length).toBeGreaterThan(30);
  });

  it("every one is registered with the ground it is drawn on", () => {
    const unregistered = directRules.filter((rule) => entriesFor(rule).length !== 1).map(label);
    expect(unregistered).toEqual([]);
  });

  it("no registered ground stays dark in both themes, and the accent reads on each", () => {
    const failures: string[] = [];
    for (const entry of DIRECT_RING_GROUNDS) {
      if (DARK_IN_BOTH_THEMES.has(entry.ground)) {
        failures.push(`${entry.file}: ${entry.match} sits on ${entry.ground}, which needs --ring`);
        continue;
      }
      for (const theme of THEMES) {
        const ratio = contrastRatio(hex("--accent", theme), hex(entry.ground, theme));
        if (ratio < RING_FLOOR) {
          failures.push(
            `${entry.file}: ${entry.match} (${theme}) --accent on ${entry.ground} is ${ratio.toFixed(2)}`,
          );
        }
      }
    }
    expect(failures).toEqual([]);
  });

  it("holds the registry to rules that still read the accent directly", () => {
    const live = new Set(directRules.flatMap((rule) => entriesFor(rule)));
    const stale = DIRECT_RING_GROUNDS.filter((entry) => !live.has(entry));
    expect(stale.map((entry) => `${entry.file}: ${entry.match}`)).toEqual([]);
  });
});
