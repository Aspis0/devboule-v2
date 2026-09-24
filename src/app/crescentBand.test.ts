import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// The two boxes are read out of the stylesheets, not the DOM: no layout runs
// in these tests, so a getBoundingClientRect here would be a zero rectangle
// for both boxes and the overlap would pass unmeasured.
const globalCss = readFileSync(new URL("../styles/global.css", import.meta.url), "utf8");
const tokensCss = readFileSync(new URL("../styles/tokens.css", import.meta.url), "utf8");

function ruleBody(selector: string): string {
  const escaped = selector.replaceAll(".", "\\.");
  const match = new RegExp(`${escaped}\\s*\\{([\\s\\S]*?)\\n\\}`).exec(globalCss);
  if (match === null) throw new Error(`no rule for ${selector}`);
  return match[1]!;
}

/** A declaration's pixel length, with var() resolved through tokens.css. */
function pixels(body: string, property: string): number | undefined {
  const declared = new RegExp(`^\\s*${property}:\\s*([^;]+);`, "m").exec(body)?.[1]?.trim();
  if (declared === undefined) return undefined;
  const value = declared.replace(/var\((--[a-z0-9-]+)\)/, (_reference, name: string) => {
    const token = new RegExp(`${name}:\\s*([^;]+);`).exec(tokensCss)?.[1]?.trim();
    if (token === undefined) throw new Error(`undefined token ${name}`);
    return token;
  });
  if (value === "0") return 0;
  const px = /^(-?\d+(?:\.\d+)?)px$/.exec(value);
  if (px === null) throw new Error(`${property}: ${declared} is not a pixel length`);
  return Number(px[1]);
}

function declared(body: string, property: string): number {
  const value = pixels(body, property);
  if (value === undefined) throw new Error(`${property} is not declared`);
  return value;
}

describe("the band the crescent's sliver lives in", () => {
  it("holds the whole sliver, so the surfaces below it share no pixel with it", () => {
    const crescentShell = ruleBody(".crescent-shell");
    const sliver = ruleBody(".crescent-sliver");
    const appShell = ruleBody(".app-shell");
    const pageLayer = ruleBody(".page-layer");

    // The sliver's box: the crescent shell hangs off the window's top edge and
    // the sliver off the top of the crescent shell.
    const sliverBottom =
      declared(crescentShell, "top") + declared(sliver, "top") + declared(sliver, "height");

    // The band is the shell's own top padding; every surface is laid out in the
    // page layer, which starts at the band's bottom plus its own top margin.
    const band = pixels(appShell, "padding-top") ?? 0;
    const surfaceTop = band + (pixels(pageLayer, "margin-top") ?? 0);

    expect(sliverBottom, "the sliver must end inside the band").toBeLessThanOrEqual(band);
    expect(
      surfaceTop,
      "the surface must start at or below the band's bottom",
    ).toBeGreaterThanOrEqual(band);
  });
});
