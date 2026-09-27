// @vitest-environment happy-dom

// The Oracle surface against the settings card language, proved against
// the real stylesheets in the REAL bundle order, measured with
// `vite build` on this tree (SettingsSurface chunk byte offsets:
// diagnostics 55, devices 3427, oracle 7656, general 25685, providers
// 26993, profiles 33013, projects 38259, settings.css LAST at 39720+).
// Bare single-class selectors in the light theme only (cssProof's scope);
// the dark theme and anything it cannot see belong to a live check and
// are listed in the slice report. The four-step flow and every Oracle IPC
// stay exactly as they are — the flow pin is a markup test in
// OracleSetup.test.tsx, not a CSS string below.

import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

function box(className: string): HTMLElement {
  const el = document.createElement("div");
  el.className = className;
  document.body.appendChild(el);
  return el;
}

const proof = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/settings/diagnostics.css"),
  read("src/features/settings/devices.css"),
  read("src/features/oracle/oracle.css"),
  read("src/features/settings/general.css"),
  read("src/features/settings/providers.css"),
  read("src/features/settings/profiles.css"),
  read("src/features/settings/projects.css"),
  read("src/features/settings/settings.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("oracle cards (real stylesheets, no app launch)", () => {
  it("grounds the setup flow on the house card", () => {
    proof.inject([".oracle-stage-card"]);
    const card = box("oracle-stage-card");
    const style = getComputedStyle(card);
    expect(style.maxWidth).toBe("720px");
    expect(style.borderRadius).toBe("12px");
    // The ground and edge already resolve through the shell aliases
    // (--surface → --panel-card, --border → --line); pin the resolution
    // so a token edit cannot silently move the card off the house.
    expect(proof.rulesFor(".oracle-stage-card")).toContain(proof.token("--panel-card"));
    expect(proof.rulesFor(".oracle-stage-card")).toContain(proof.token("--line"));
    expect(proof.rulesFor(".oracle-stage-card")).not.toContain("box-shadow");
  });

  it("grounds the ask surface on the same card, without its own shadow", () => {
    proof.inject([".oracle-query-surface"]);
    const surface = box("oracle-query-surface");
    expect(getComputedStyle(surface).borderRadius).toBe("12px");
    expect(proof.rulesFor(".oracle-query-surface")).toContain(proof.token("--panel-card"));
    expect(proof.rulesFor(".oracle-query-surface")).not.toContain("box-shadow");
  });

  it("keeps Oracle titles, eyebrows, and body copy in the card hierarchy", () => {
    const selectors = [
      ".oracle-stage-content h3",
      ".oracle-ready-intro h3",
      ".oracle-admin-block[aria-labelledby] h4",
      ".oracle-admin-block h4",
      ".oracle-files-heading h4",
      ".oracle-results-heading h3",
      ".oracle-eyebrow",
      ".oracle-stage-description",
      ".oracle-ready-intro p",
    ];
    proof.inject(selectors);

    const stage = document.createElement("div");
    stage.className = "oracle-stage-content";
    const eyebrow = document.createElement("div");
    eyebrow.className = "oracle-eyebrow";
    const title = document.createElement("h3");
    const body = document.createElement("p");
    body.className = "oracle-stage-description";
    stage.append(eyebrow, title, body);
    document.body.appendChild(stage);

    const ready = document.createElement("div");
    ready.className = "oracle-ready-intro";
    const readyTitle = document.createElement("h3");
    const readyBody = document.createElement("p");
    ready.append(readyTitle, readyBody);
    document.body.appendChild(ready);

    const admin = document.createElement("section");
    admin.className = "oracle-admin-block";
    admin.setAttribute("aria-labelledby", "admin-title");
    const adminTitle = document.createElement("h4");
    admin.appendChild(adminTitle);
    document.body.appendChild(admin);

    const filesHeading = document.createElement("div");
    filesHeading.className = "oracle-files-heading";
    const filesTitle = document.createElement("h4");
    filesHeading.appendChild(filesTitle);
    document.body.appendChild(filesHeading);

    const resultsHeading = document.createElement("div");
    resultsHeading.className = "oracle-results-heading";
    const resultsTitle = document.createElement("h3");
    resultsHeading.appendChild(resultsTitle);
    document.body.appendChild(resultsHeading);

    for (const heading of [title, readyTitle, adminTitle, filesTitle, resultsTitle]) {
      const style = getComputedStyle(heading);
      expect(style.fontSize).toBe("14px");
      expect(style.fontWeight).toBe("600");
      expect(style.color).toBe(proof.token("--ink"));
    }
    const eyebrowStyle = getComputedStyle(eyebrow);
    expect(eyebrowStyle.fontSize).toBe("12px");
    expect(eyebrowStyle.fontWeight).toBe("500");
    expect(eyebrowStyle.color).toBe(proof.token("--muted"));
    for (const paragraph of [body, readyBody]) {
      const bodyStyle = getComputedStyle(paragraph);
      expect(bodyStyle.fontSize).toBe("13px");
      expect(bodyStyle.color).toBe(proof.token("--ink-soft"));
    }
  });

  it("carries no page heading of its own — the shell titles the page", () => {
    // Two halves: no rule declares one, and no component renders one. A
    // resurrected consumer would otherwise render an unstyled heading
    // with the first half still green.
    expect(read("src/features/oracle/oracle.css")).not.toContain("oracle-page-heading");
    const dir = resolve(rootDir, "src/features/oracle");
    for (const file of readdirSync(dir)) {
      if (!file.endsWith(".tsx") || file.includes(".test.")) continue;
      expect(read(`src/features/oracle/${file}`)).not.toContain("oracle-page-heading");
    }
  });

  it("labels each admin block through the heading referenced by aria-labelledby", () => {
    const source = read("src/features/oracle/OracleAdmin.tsx");
    const blocks = [...source.matchAll(/<section\b[^>]*className="oracle-admin-block"[^>]*>/g)];
    expect(blocks).toHaveLength(3);
    for (const [index, match] of blocks.entries()) {
      const opening = match[0]!;
      const labelId = opening.match(/aria-labelledby="([^"]+)"/)?.[1];
      expect(labelId, `admin block ${index + 1} needs aria-labelledby`).toBeDefined();
      const end = source.indexOf("</section>", match.index);
      const content = source.slice(match.index, end);
      expect(content).toContain(`<h4 id="${labelId}"`);
    }
  });

  it("declares the file inventory heading in one rule", () => {
    const css = read("src/features/oracle/oracle.css");
    expect([...css.matchAll(/\.oracle-files-heading h4/g)]).toHaveLength(1);
  });

  it("keeps the four-step rail and its step states", () => {
    const css = read("src/features/oracle/oracle.css");
    expect(css).toContain(".oracle-setup-rail");
    expect(css).toContain(".oracle-setup-step-current");
    expect(css).toContain(".oracle-setup-step-done");
  });
});
