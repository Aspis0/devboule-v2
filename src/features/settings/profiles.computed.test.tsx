// @vitest-environment happy-dom

// The Agents rows and the profile dialog against the real stylesheets: the
// assembled sheets in the bundle's own order — tokens, global (main.tsx),
// then the lazy settings chunk in ESM evaluation order, which is profiles
// (imported through AgentsPanel at SettingsSurface.tsx:12) before settings
// (SettingsSurface.tsx:23). Bare single-class selectors only: that is what
// the cssProof harness injects.
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

describe("theme roots (real stylesheet, static)", () => {
  const { rulesFor } = assembleCssProof([read("src/styles/tokens.css")]);

  it("pins color-scheme on both theme roots", () => {
    // Native controls (the dialog's selects, checkboxes, scrollbars) take
    // their light/dark rendering from here; without it the dark theme
    // renders white OS controls. Static by necessity: happy-dom computes
    // no color-scheme.
    expect(rulesFor(":root")).toContain("color-scheme: light");
    expect(rulesFor('[data-theme="dark"]')).toContain("color-scheme: dark");
  });
});

describe("agent profile rows and dialog (real stylesheets, no app launch)", () => {
  const { inject, rulesFor, token } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/settings/profiles.css"),
    read("src/features/settings/settings.css"),
  ]);

  it("draws the glyph tile 28 px with an 8 px radius", () => {
    inject([".profile-tile"]);
    const tile = box("profile-tile");
    const style = getComputedStyle(tile);
    expect(style.width).toBe("28px");
    expect(style.height).toBe("28px");
    expect(style.borderRadius).toBe("8px");
  });

  it("gives the main column the row's growth, gap and min-width", () => {
    inject([".agent-profile-main"]);
    const main = box("agent-profile-main");
    const style = getComputedStyle(main);
    expect(style.display).toBe("grid");
    expect(style.flexGrow).toBe("1");
    expect(style.flexBasis).toBe("260px");
    expect(style.gap).toBe("4px");
    expect(style.minWidth).toBe("0");
  });

  it("sets the row type to the spec's sizes", () => {
    // The name carries no other sizing class: `.settings-card-title` (13 px
    // in settings.css, which is later in the chunk) is not on the element,
    // so 14 px here is won, not unopposed-by-absence.
    inject([".profile-name", ".profile-meta", ".profile-spawn"]);
    expect(getComputedStyle(box("profile-name")).fontSize).toBe("14px");
    expect(getComputedStyle(box("profile-meta")).fontSize).toBe("12px");
    expect(getComputedStyle(box("profile-spawn")).fontSize).toBe("13px");
  });

  it("lays the pen beside the spawn text and clamps the text to two lines", () => {
    inject([".profile-spawn", ".profile-spawn-text"]);
    const row = box("profile-spawn");
    expect(getComputedStyle(row).display).toBe("flex");
    const text = box("profile-spawn-text");
    expect(getComputedStyle(text).overflow).toBe("hidden");
    // happy-dom does not compute `-webkit-line-clamp` (undefined above),
    // so the two-line budget is read from the assembled source — stated as
    // a source check, not a computed value.
    expect(rulesFor(".profile-spawn-text")).toContain("-webkit-line-clamp: 2");
  });

  it("sizes the row's icon buttons 26 px and dims the dead ends", () => {
    inject([".profile-icon-btn", ".profile-icon-btn:disabled", ".profile-icon-btn.profile-is-dim"]);
    const button = box("profile-icon-btn");
    expect(getComputedStyle(button).width).toBe("26px");
    expect(getComputedStyle(button).height).toBe("26px");
    const dimmed = document.createElement("button");
    dimmed.className = "profile-icon-btn profile-is-dim";
    document.body.appendChild(dimmed);
    expect(getComputedStyle(dimmed).opacity).toBe("0.4");
    // Both rules on one disabled button: the dead-end dim wins over the
    // generic disabled treatment.
    const deadEnd = document.createElement("button");
    deadEnd.className = "profile-icon-btn profile-is-dim";
    deadEnd.disabled = true;
    document.body.appendChild(deadEnd);
    expect(getComputedStyle(deadEnd).opacity).toBe("0.4");
    const busyPencil = document.createElement("button");
    busyPencil.className = "profile-icon-btn";
    busyPencil.disabled = true;
    document.body.appendChild(busyPencil);
    expect(getComputedStyle(busyPencil).opacity).toBe("0.55");
  });

  it("paints the delete action in the danger colour", () => {
    inject([".profile-icon-btn", ".profile-icon-btn-trash"]);
    const trash = box("profile-icon-btn profile-icon-btn-trash");
    expect(getComputedStyle(trash).color).toBe(token("--danger"));
  });

  it("covers the window with the shared modal dim and centres the dialog", () => {
    inject([".edit-scrim"]);
    const scrim = box("edit-scrim");
    const style = getComputedStyle(scrim);
    expect(style.position).toBe("fixed");
    expect(style.display).toBe("grid");
    // Every dialog dims the page the same way: the shared modal dim, not
    // the lightbox scrim.
    expect(style.backgroundColor).toBe(token("--modal-dim"));
  });

  it("styles the dialog's textareas like the pane's", () => {
    inject([".agent-profiles textarea", ".edit-card textarea"]);
    const card = box("edit-card");
    const field = document.createElement("textarea");
    card.appendChild(field);
    const inCard = getComputedStyle(field);
    const pane = document.createElement("div");
    pane.className = "agent-profiles";
    const standing = document.createElement("textarea");
    pane.appendChild(standing);
    document.body.appendChild(pane);
    const inPane = getComputedStyle(standing);
    // Parity first: whatever the values are, both contexts compute them.
    for (const property of ["paddingTop", "borderRadius", "fontSize", "lineHeight"]) {
      expect(inCard[property as "paddingTop"]).toBe(inPane[property as "paddingTop"]);
    }
    // Then the values themselves: parity at 40 px would pass above and
    // still be wrong, so the sheet's own numbers are pinned here.
    expect(inCard.paddingTop).toBe("8px");
    expect(inCard.borderRadius).toBe("8px");
    expect(inCard.fontSize).toBe("12px");
  });

  it("holds the dialog card at 480 px with a 12 px radius, scrolling inside", () => {
    inject([".edit-card"]);
    const card = box("edit-card");
    const style = getComputedStyle(card);
    expect(style.width).toBe("480px");
    expect(style.borderRadius).toBe("12px");
    expect(style.overflowY).toBe("auto");
  });

  it("clears the shared label's top margin inside the delegation box", () => {
    // Descendant selectors never inject, so this is a source check, stated
    // as one: the 26 px `.settings-subheading` margin would otherwise paint
    // ~40 px of empty box-top (12 px padding + 26 px margin, grid keeps both).
    const { rulesFor: boxRules } = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/settings/profiles.css"),
      read("src/features/settings/settings.css"),
    ]);
    expect(boxRules(".agent-delegation > :first-child")).toContain("margin-top: 0");
  });

  it("gives the card the app's thin scrollbars, not the native bar", () => {
    inject([".edit-card"]);
    const style = getComputedStyle(box("edit-card"));
    expect(style.scrollbarWidth).toBe("thin");
    expect(style.scrollbarColor).not.toBe("");
  });

  it("pins the form's actions to the card's bottom edge", () => {
    inject([".profile-form-actions"]);
    const actions = box("profile-form-actions");
    const style = getComputedStyle(actions);
    expect(style.position).toBe("sticky");
    expect(style.bottom).toBe("-16px");
    expect(style.backgroundColor).toBe(token("--panel-card"));
  });
});
