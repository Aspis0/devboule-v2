// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../features/workspace/cssProof";

const rootDir = resolve(import.meta.dirname, "../..");
const read = (path: string) => readFileSync(resolve(rootDir, path), "utf8");
// Each assertion injects rules from the stylesheet that owns those selectors.
// This verifies declared styles without claiming a cross-file bundle order.
const cardCss = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/components/PermissionCard.css"),
]);
const cardCssDark = assembleCssProof(
  [read("src/styles/tokens.css"), read("src/components/PermissionCard.css")],
  "dark",
);

afterEach(removeCssProof);

describe("PermissionCard computed styles", () => {
  it("gives the card the mockup shell: panel fill, hairline, r12, 11/14 padding", () => {
    cardCss.inject([".permission-card"]);
    const card = document.createElement("div");
    card.className = "permission-card";
    document.body.appendChild(card);
    const style = getComputedStyle(card);
    expect(style.backgroundColor).toBe(cardCss.token("--panel-card"));
    expect(style.borderRadius).toBe("12px");
    expect(style.padding).toBe("11px 14px");
    // The hairline: the rule source carries the resolved token.
    expect(cardCss.rulesFor(".permission-card")).toContain("border: 1px solid #ded6c4");
    // The design surface's notice box shares the sheet and the ramp's floor.
    expect(cardCss.rulesFor(".permission-card-notice")).toContain("font-size: 12px");
    card.remove();
  });

  it("sets the head at 12/500 muted with an 8 px gap, and the action line at 14 px ink", () => {
    cardCss.inject([
      ".permission-card-heading",
      ".permission-card-title",
      ".permission-card > .permission-card-action",
    ]);
    const head = document.createElement("div");
    head.className = "permission-card-heading";
    const title = document.createElement("span");
    title.className = "permission-card-title";
    // The action line's rule is scoped to a direct child of the card.
    const card = document.createElement("div");
    card.className = "permission-card";
    const action = document.createElement("div");
    action.className = "permission-card-action";
    card.appendChild(action);
    document.body.append(head, title, card);
    const headStyle = getComputedStyle(head);
    expect(headStyle.fontSize).toBe("12px");
    expect(headStyle.fontWeight).toBe("500");
    expect(headStyle.gap).toBe("8px");
    expect(getComputedStyle(title).color).toBe(cardCss.token("--muted"));
    const actionStyle = getComputedStyle(action);
    expect(actionStyle.fontSize).toBe("14px");
    expect(actionStyle.fontWeight).toBe("600");
    expect(actionStyle.color).toBe(cardCss.token("--ink"));
    head.remove();
    title.remove();
    card.remove();
  });

  it("puts the command on the code ground at mono 12.5, r6, 6/10 padding", () => {
    cardCss.inject([".permission-card-command"]);
    const command = document.createElement("div");
    command.className = "permission-card-command";
    document.body.appendChild(command);
    const style = getComputedStyle(command);
    expect(style.backgroundColor).toBe(cardCss.token("--code-bg"));
    expect(style.color).toBe(cardCss.token("--code-text"));
    expect(style.fontFamily).toContain("JetBrains Mono");
    expect(style.fontSize).toBe("12.5px");
    expect(style.borderRadius).toBe("6px");
    expect(style.padding).toBe("6px 10px");
    command.remove();
  });

  it("gives the option chips the mockup geometry and the chosen state its accent trio", () => {
    cardCss.inject([".permission-card-question-option", ".permission-card-question-option-chosen"]);
    const chip = document.createElement("div");
    chip.className = "permission-card-question-option";
    const chosen = document.createElement("div");
    chosen.className = "permission-card-question-option permission-card-question-option-chosen";
    document.body.append(chip, chosen);
    const chipStyle = getComputedStyle(chip);
    expect(chipStyle.minHeight).toBe("28px");
    // No fixed height: the chip grows with a wrapped description. The rule
    // source is the honest expression of that intent — a computed-height
    // assertion would encode a happy-dom quirk, not the intent.
    expect(cardCss.rulesFor(".permission-card-question-option")).toContain("min-height: 28px");
    expect(chipStyle.borderRadius).toBe("6px");
    expect(chipStyle.fontSize).toBe("13px");
    expect(chipStyle.color).toBe(cardCss.token("--ink-soft"));
    expect(chipStyle.padding).toBe("4px 10px");
    const chosenStyle = getComputedStyle(chosen);
    expect(chosenStyle.borderColor).toBe(cardCss.token("--accent"));
    expect(chosenStyle.color).toBe(cardCss.token("--ink"));
    // The accent-soft fill is a color-mix the DOM engine does not resolve;
    // the rule source carries it, with the token resolved.
    expect(cardCss.rulesFor(".permission-card-question-option-chosen")).toContain(
      "background: color-mix(in srgb, #bd4a26 10%, transparent)",
    );
    chip.remove();
    chosen.remove();
  });

  it("pairs an outline Deny with a filled Allow once, both h28 r6 13px", () => {
    cardCss.inject([".permission-card-secondary-action", ".permission-card-primary-action"]);
    const deny = document.createElement("button");
    deny.className = "permission-card-secondary-action permission-card-deny-action";
    const allow = document.createElement("button");
    allow.className = "permission-card-primary-action";
    document.body.append(deny, allow);
    const denyStyle = getComputedStyle(deny);
    expect(denyStyle.height).toBe("28px");
    expect(denyStyle.borderRadius).toBe("6px");
    expect(denyStyle.fontSize).toBe("13px");
    expect(denyStyle.color).toBe(cardCss.token("--ink-soft"));
    expect(denyStyle.backgroundColor).toBe("transparent");
    const allowStyle = getComputedStyle(allow);
    expect(allowStyle.height).toBe("28px");
    expect(allowStyle.backgroundColor).toBe(cardCss.token("--accent"));
    expect(allowStyle.color).toBe(cardCss.token("--accent-contrast"));
    deny.remove();
    allow.remove();
  });

  it("carries the shell, the command ground and the chosen accent into the dark theme", () => {
    cardCssDark.inject([
      ".permission-card",
      ".permission-card-command",
      ".permission-card-question-option-chosen",
      ".permission-card-primary-action",
    ]);
    const card = document.createElement("div");
    card.className = "permission-card";
    const command = document.createElement("div");
    command.className = "permission-card-command";
    const chosen = document.createElement("div");
    chosen.className = "permission-card-question-option permission-card-question-option-chosen";
    const allow = document.createElement("button");
    allow.className = "permission-card-primary-action";
    document.body.append(card, command, chosen, allow);
    expect(getComputedStyle(card).backgroundColor).toBe(cardCssDark.token("--panel-card"));
    expect(getComputedStyle(command).backgroundColor).toBe(cardCssDark.token("--code-bg"));
    expect(getComputedStyle(chosen).borderColor).toBe(cardCssDark.token("--accent"));
    expect(getComputedStyle(allow).backgroundColor).toBe(cardCssDark.token("--accent"));
    expect(getComputedStyle(allow).color).toBe(cardCssDark.token("--accent-contrast"));
    card.remove();
    command.remove();
    chosen.remove();
    allow.remove();
  });
});
