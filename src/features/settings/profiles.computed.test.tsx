// @vitest-environment happy-dom

// The Agents rows and the profile dialog against the real stylesheets: the
// assembled sheets in bundle order (tokens, global, settings, profiles —
// main.tsx first, the lazy settings chunk after), so a geometry regression
// against SPEC-regions.md §Settings fails here, not live. Bare single-class
// selectors only: that is what the cssProof harness injects.
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

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("agent profile rows and dialog (real stylesheets, no app launch)", () => {
  const { inject, rulesFor, token } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/settings.css"),
    read("src/features/settings/profiles.css"),
  ]);

  it("draws the glyph tile 28 px with an 8 px radius", () => {
    inject([".profile-tile"]);
    const tile = box("profile-tile");
    const style = getComputedStyle(tile);
    expect(style.width).toBe("28px");
    expect(style.height).toBe("28px");
    expect(style.borderRadius).toBe("8px");
  });

  it("sets the row type to the spec's sizes", () => {
    inject([".profile-name", ".profile-meta", ".profile-spawn"]);
    expect(getComputedStyle(box("profile-name")).fontSize).toBe("14px");
    expect(getComputedStyle(box("profile-meta")).fontSize).toBe("12px");
    expect(getComputedStyle(box("profile-spawn")).fontSize).toBe("13px");
  });

  it("clamps the row's spawn prompt to two lines", () => {
    inject([".profile-spawn"]);
    expect(getComputedStyle(box("profile-spawn")).overflow).toBe("hidden");
    const source = rulesFor(".profile-spawn");
    expect(source).toContain("-webkit-line-clamp: 2");
  });

  it("sizes the row's icon buttons 26 px and dims the dead ends", () => {
    inject([".profile-icon-btn", ".is-dim"]);
    const button = box("profile-icon-btn");
    expect(getComputedStyle(button).width).toBe("26px");
    expect(getComputedStyle(button).height).toBe("26px");
    expect(getComputedStyle(box("is-dim")).opacity).toBe("0.4");
  });

  it("paints the delete action in the danger colour", () => {
    inject([".profile-icon-btn", ".profile-icon-btn-trash"]);
    const danger = token("--danger");
    expect(danger).not.toBeUndefined();
    expect(rulesFor(".profile-icon-btn-trash")).toContain(danger);
  });

  it("covers the window with the scrim and centres the dialog", () => {
    inject([".edit-scrim"]);
    const scrim = box("edit-scrim");
    const style = getComputedStyle(scrim);
    expect(style.position).toBe("fixed");
    expect(style.display).toBe("grid");
    expect(style.backgroundColor).toBe(token("--scrim"));
  });

  it("holds the dialog card at 480 px with a 14 px radius", () => {
    inject([".edit-card"]);
    const card = box("edit-card");
    const style = getComputedStyle(card);
    expect(style.width).toBe("480px");
    expect(style.borderRadius).toBe("14px");
  });
});
