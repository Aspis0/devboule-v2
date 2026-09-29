// The type floor walk's own contract, on synthetic sheets: resolution of
// tokens, aliases and fallbacks; conditional at-rules; relative units; and
// the shorthand and math forms. The slice's real sheets are policed in
// typeFloor.workspace.test.ts.
// @vitest-environment node
import { describe, expect, it } from "vitest";
import { TYPE_FLOOR_PX, findBelowTypeFloor } from "./typeFloor";

const ROOT = ":root { --type-meta: 12px; }";

function one(css: string) {
  const findings = findBelowTypeFloor([css]);
  expect(findings).toHaveLength(1);
  return findings[0]!;
}

describe("token resolution", () => {
  it("resolves a var() to its fallback and judges the fallback", () => {
    expect(one(`${ROOT} .probe { font-size: var(--type-nope, 11px); }`)).toMatchObject({
      px: 11,
      declaration: "font-size: 11px",
    });
  });

  it("fails an unknown custom property instead of skipping it", () => {
    expect(one(`${ROOT} .probe { font-size: var(--type-met); }`)).toMatchObject({
      px: null,
      reason: expect.stringContaining("unknown custom property --type-met"),
    });
  });

  it("resolves alias chains of any depth", () => {
    const chain =
      ":root { --a5: 11px; --a4: var(--a5); --a3: var(--a4); --a2: var(--a3); --a1: var(--a2); }";
    expect(one(`${chain} .probe { font-size: var(--a1); }`)).toMatchObject({ px: 11 });
  });

  it("fails a cyclic alias chain instead of hanging", () => {
    expect(
      one(`${ROOT} :root { --x: var(--y); --y: var(--x); } .probe { font-size: var(--x); }`),
    ).toMatchObject({
      px: null,
      reason: expect.stringContaining("cyclic"),
    });
  });
});

describe("conditional at-rules", () => {
  for (const at of [
    "@media (max-width: 900px)",
    "@supports (display: grid)",
    "@container (min-width: 100px)",
  ]) {
    it(`walks rules inside ${at.split(" ")[0]}`, () => {
      expect(one(`${at} { .probe { font-size: 9px; } }`)).toMatchObject({
        rule: ".probe",
        px: 9,
      });
    });
  }
});

describe("relative units", () => {
  it("resolves rem and % against the 16px root", () => {
    expect(one(".probe { font-size: 0.7rem; }")).toMatchObject({ px: 11.2 });
    expect(one(".probe { font-size: 70%; }")).toMatchObject({ px: 11.2 });
    expect(findBelowTypeFloor([".probe { font-size: 1rem; }"])).toEqual([]);
  });

  it("scales em by the parent the same sheet proves", () => {
    const css = ".parent { font-size: 12px; } .parent .probe { font-size: 0.9em; }";
    expect(one(css)).toMatchObject({ px: 10.8 });
  });

  it("fails an em whose named parent has no size in the sheet", () => {
    expect(one(".section .probe { font-size: 0.9em; }")).toMatchObject({
      px: null,
      reason: expect.stringContaining(
        'no rule in the same sheet sets a font-size for the parent ".section"',
      ),
    });
  });

  it("fails an em on a rule that names no parent at all", () => {
    expect(one(".probe { font-size: 0.9em; }")).toMatchObject({
      px: null,
      reason: expect.stringContaining("names no parent"),
    });
  });
});

describe("shorthand and math forms", () => {
  it("extracts the size from a font shorthand", () => {
    expect(one(".probe { font: italic 11px/1.4 Inter, sans-serif; }")).toMatchObject({
      px: 11,
      declaration: "font: italic 11px/1.4 Inter, sans-serif",
    });
  });

  it("judges the smallest value a math form can produce", () => {
    expect(one(".probe { font-size: clamp(8px, 1vw, 14px); }")).toMatchObject({ px: 8 });
    expect(one(".probe { font-size: min(9px, 20px); }")).toMatchObject({ px: 9 });
    expect(one(".probe { font-size: max(9px, 2vw); }")).toMatchObject({ px: 9 });
    expect(findBelowTypeFloor([".probe { font-size: clamp(14px, 2vw, 20px); }"])).toEqual([]);
  });

  it("fails a math form whose smallest value is unknowable", () => {
    expect(one(".probe { font-size: min(9px, 2vw); }")).toMatchObject({
      px: null,
      reason: expect.stringContaining("not a size the walk can resolve"),
    });
  });

  it("reads uppercase units", () => {
    expect(one(".probe { font-size: 9PX; }")).toMatchObject({ px: 9 });
  });

  it("keeps sizes at or above the floor out of the findings", () => {
    expect(findBelowTypeFloor([`.probe { font-size: ${TYPE_FLOOR_PX}px; }`])).toEqual([]);
  });
});
