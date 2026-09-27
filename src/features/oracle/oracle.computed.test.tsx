// @vitest-environment happy-dom

// The Oracle surface against the settings card language, proved against
// the real stylesheets in bundle order: tokens, global, the shell sheet,
// then oracle.css. Bare single-class selectors in the light theme only
// (cssProof's scope); the dark theme and anything it cannot see belong to
// a live check and are listed in the slice report. The four-step flow and
// every Oracle IPC stay exactly as they are — this suite pins surfaces.

import { readFileSync } from "node:fs";
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
  read("src/features/settings/settings.css"),
  read("src/features/oracle/oracle.css"),
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

  it("keeps one content-title scale under the shell's page title", () => {
    proof.inject([".oracle-stage-content h3"]);
    const content = box("oracle-stage-content");
    const title = document.createElement("h3");
    title.textContent = "Choose a folder for Oracle";
    content.appendChild(title);
    expect(getComputedStyle(title).fontSize).toBe("18px");
    // The group rule above also names this selector; what matters is the
    // cascade winner — the last font-size the sheets declare for it.
    const sizes = [
      ...proof.rulesFor(".oracle-ready-intro h3").matchAll(/font-size:\s*(\d+)px/g),
    ].map((match) => match[1]);
    expect(sizes.at(-1)).toBe("18");
  });

  it("carries no page heading of its own — the shell titles the page", () => {
    expect(read("src/features/oracle/oracle.css")).not.toContain("oracle-page-heading");
  });

  it("keeps the four-step rail and its step states", () => {
    const css = read("src/features/oracle/oracle.css");
    expect(css).toContain(".oracle-setup-rail");
    expect(css).toContain(".oracle-setup-step-current");
    expect(css).toContain(".oracle-setup-step-done");
  });
});
