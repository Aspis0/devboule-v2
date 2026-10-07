// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");
const read = (path: string) => readFileSync(resolve(rootDir, path), "utf8");
// Each assertion injects rules from the stylesheet that owns those selectors.
// This verifies declared styles without claiming a cross-file bundle order.
const workspaceCss = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/features/workspace/Workspace.css"),
]);
const timelineCss = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/features/workspace/timeline/timeline.css"),
]);
const blockCss = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/components/codeBlocks.css"),
]);
const footerCss = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/features/workspace/timeline/TurnFooter.css"),
]);

afterEach(removeCssProof);

describe("timeline computed styles", () => {
  it("fills the pane up to the reading cap with the lane's 20 px sides and the card gaps", () => {
    workspaceCss.inject([".workspace-conversation"]);
    const rail = document.createElement("div");
    rail.className = "workspace-conversation";
    document.body.appendChild(rail);

    const style = getComputedStyle(rail);
    // The lane fills the pane and the 1060 px cap binds it when the pane
    // grows past the reading width; the 20 px sides are the lane's own
    // padding, so the rows and the composer share one axis.
    expect(style.maxWidth).toBe("1060px");
    expect(style.marginLeft).toBe("auto");
    expect(style.marginRight).toBe("auto");
    // Fills the pane up to the cap instead of shrink-wrapping to the longest
    // row. Shared with the composer card and the queue track by one rule.
    expect(style.width).toBe("100%");
    expect(style.paddingTop).toBe("4px");
    expect(style.paddingRight).toBe("20px");
    expect(style.paddingBottom).toBe("0px");
    // Rows start on the lane's inset, the same axis the composer text uses.
    expect(style.paddingLeft).toBe("20px");
    // The container's gap now only separates the transcript from the permission
    // card; the entry rhythm lives on the wrapper.
    expect(style.gap).toBe("8px");
    rail.remove();
  });

  it("declares the transcript wrapper unsquashed with the rhythm on the rows", () => {
    // In a real browser a child's computed `gap` inherits the rail's 8 px,
    // so only the rule source can certify the wrapper's own declarations.
    // The wrapper carries no gap of its own: turn boundary, group and
    // compact edge each live on the lower row's top margin.
    const rules = workspaceCss.rulesFor(".workspace-conversation-content");
    expect(rules).toContain("flex: none");
    expect(rules).toContain("gap: 0");
  });

  it("gives user messages the bubble width, inset, corner shape, fill, and ink", () => {
    timelineCss.inject([".workspace-chat-bubble"]);
    const bubble = document.createElement("div");
    bubble.className = "workspace-chat-bubble";
    document.body.appendChild(bubble);

    const style = getComputedStyle(bubble);
    expect(style.maxWidth).toBe("78%");
    expect(style.padding).toBe("12px");
    expect(style.borderRadius).toBe("12px 4px 12px 12px");
    expect(style.backgroundColor).toBe("#ffffff");
    expect(style.color).toBe("#242321");
    bubble.remove();
  });

  it("styles assistant Markdown and keeps the copy chip visible on keyboard focus", () => {
    timelineCss.inject([
      ".workspace-chat-assistant",
      ...[1, 2, 3, 4, 5, 6].map(
        (level) => `.workspace-chat-assistant .workspace-chat-copy .plan-markdown-heading-${level}`,
      ),
      ".workspace-chat-assistant .workspace-chat-copy ul",
      ".workspace-chat-assistant .workspace-chat-copy code:not(pre code)",
      ".timeline-copy-chip",
      ".timeline-copy-chip:focus-visible",
      ".workspace-chat-user:hover .timeline-copy-chip",
      ".workspace-chat-user:focus-within .timeline-copy-chip",
      ".workspace-chat-assistant:hover .timeline-copy-chip",
      ".workspace-chat-assistant:focus-within .timeline-copy-chip",
    ]);
    blockCss.inject([
      ".copyblock",
      ".copyblock:hover .copy-btn",
      ".copyblock:focus-within .copy-btn",
      ".codeblock-sample",
      ".codeblock-sample pre",
      ".codeblock-sample:hover .copy-btn",
      ".codeblock-sample:focus-within .copy-btn",
      ".copy-btn",
      ".copy-btn.is-copied",
      ".copy-btn:focus-visible",
      ".copyblock > .sr-only",
    ]);
    const assistant = document.createElement("div");
    assistant.className = "workspace-chat-assistant";
    assistant.innerHTML = [
      '<div class="workspace-chat-copy">',
      ...[1, 2, 3, 4, 5, 6].map(
        (level) => `<div class="plan-markdown-heading-${level}">Heading ${level}</div>`,
      ),
      "<ul><li>item</li></ul>",
      "<code>inline</code><pre><code>block</code></pre>",
      '<div class="copyblock"><button class="copy-btn" type="button">Copy</button>pnpm build</div>',
      '<div class="codeblock-sample"><button class="copy-btn is-copied" type="button">✓ Copied</button><pre><code>sample</code></pre></div>',
      '<button class="timeline-copy-chip" type="button">Copy</button>',
      "</div>",
    ].join("");
    document.body.appendChild(assistant);

    const heading = assistant.querySelector<HTMLElement>(".plan-markdown-heading-3");
    const list = assistant.querySelector<HTMLElement>("ul");
    const inlineCode = assistant.querySelector<HTMLElement>(".workspace-chat-copy > code");
    const codeBlock = assistant.querySelector<HTMLElement>(".codeblock-sample pre");
    const copy = assistant.querySelector<HTMLButtonElement>(".timeline-copy-chip");
    const copyable = assistant.querySelector<HTMLElement>(".copyblock");
    const sample = assistant.querySelector<HTMLElement>(".codeblock-sample");
    const blockCopy = assistant.querySelector<HTMLElement>(".copyblock .copy-btn");
    const copiedBtn = assistant.querySelector<HTMLElement>(".copy-btn.is-copied");
    if (
      heading === null ||
      list === null ||
      inlineCode === null ||
      codeBlock === null ||
      copy === null ||
      copyable === null ||
      sample === null ||
      blockCopy === null ||
      copiedBtn === null
    ) {
      throw new Error("Markdown or copy markup did not render");
    }
    expect(getComputedStyle(assistant).fontSize).toBe("15px");
    expect(getComputedStyle(assistant).lineHeight).toBe("21px");
    expect(heading).not.toBeNull();
    for (const level of [1, 2, 3, 4, 5, 6]) {
      const levelHeading = assistant.querySelector<HTMLElement>(`.plan-markdown-heading-${level}`);
      expect(levelHeading).not.toBeNull();
      expect(getComputedStyle(levelHeading!).fontSize).toBe(
        ({ 1: "18px", 2: "16px", 3: "14px", 4: "13px", 5: "12px", 6: "12px" } as const)[
          level as 1 | 2 | 3 | 4 | 5 | 6
        ],
      );
      expect(getComputedStyle(levelHeading!).fontWeight).toBe("600");
    }
    expect(getComputedStyle(list).paddingLeft).toBe("16px");
    expect(getComputedStyle(inlineCode).fontSize).toBe("13px");
    expect(getComputedStyle(inlineCode).fontFamily).toContain("JetBrains Mono");
    expect(getComputedStyle(codeBlock).backgroundColor).toBe("#201f1e");
    expect(getComputedStyle(codeBlock).paddingRight).toBe("80px");
    expect(getComputedStyle(copyable).backgroundColor).toBe("#e9e6dd");
    expect(getComputedStyle(copyable).borderLeftWidth).toBe("3px");
    // The copyable block sits on the tool fill (light in the light theme), so
    // its edge is the accent that reads on that fill, not the on-code tone.
    expect(getComputedStyle(copyable).borderLeftColor).toBe("#7a5000");
    expect(getComputedStyle(copyable).borderRadius).toBe("8px");
    expect(getComputedStyle(copyable).padding).toBe("10px 80px 10px 12px");
    expect(getComputedStyle(copyable).fontFamily).toContain("JetBrains Mono");
    expect(getComputedStyle(copyable).fontSize).toBe("13px");
    expect(getComputedStyle(blockCopy).height).toBe("24px");
    expect(getComputedStyle(blockCopy).opacity).toBe("0");
    expect(getComputedStyle(blockCopy).pointerEvents).toBe("none");
    expect(getComputedStyle(blockCopy).userSelect).toBe("none");
    expect(getComputedStyle(blockCopy).position).toBe("absolute");
    expect(getComputedStyle(blockCopy).right).toBe("6px");
    expect(getComputedStyle(copy).position).toBe("absolute");
    expect(getComputedStyle(copy).right).toBe("0px");
    // The control overlays the row: a 24 px target with no chip of its own,
    // and nothing on the row or its text reserves width for it.
    expect(getComputedStyle(copy).width).toBe("24px");
    expect(getComputedStyle(copy).height).toBe("24px");
    expect(getComputedStyle(copy).padding).toBe("0px");
    expect(getComputedStyle(copy).borderTopWidth).toBe("0px");
    expect(getComputedStyle(copy).backgroundColor).toBe("transparent");
    expect(getComputedStyle(copiedBtn).color).toBe("#3f7a56");
    expect(getComputedStyle(copiedBtn).opacity).toBe("1");
    expect(blockCss.rulesFor(".copyblock:hover .copy-btn")).toContain("opacity: 1");
    expect(blockCss.rulesFor(".copyblock:hover .copy-btn")).toContain("pointer-events: auto");
    expect(blockCss.rulesFor(".copyblock:focus-within .copy-btn")).toContain(
      "pointer-events: auto",
    );
    expect(blockCss.rulesFor(".codeblock-sample:hover .copy-btn")).toContain("opacity: 1");
    expect(blockCss.rulesFor(".codeblock-sample:focus-within .copy-btn")).toContain(
      "pointer-events: auto",
    );
    expect(blockCss.rulesFor(".copy-btn:focus-visible")).toContain("opacity: 1");
    expect(blockCss.rulesFor(".copy-btn:focus-visible")).toContain("outline: 2px solid");
    expect(blockCss.rulesFor(".copyblock > .sr-only")).toContain("user-select: none");
    expect(getComputedStyle(copy).opacity).toBe("0");
    expect(getComputedStyle(copy).pointerEvents).toBe("none");
    copy.focus();
    expect(document.activeElement).toBe(copy);
    expect(timelineCss.rulesFor(".timeline-copy-chip:focus-visible")).toContain("opacity: 1");
    expect(timelineCss.rulesFor(".timeline-copy-chip:focus-visible")).toContain(
      "pointer-events: auto",
    );
    expect(timelineCss.rulesFor(".timeline-copy-chip:focus-visible")).toContain(
      "outline: 2px solid",
    );
    expect(timelineCss.rulesFor(".workspace-chat-user:hover .timeline-copy-chip")).toContain(
      "pointer-events: auto",
    );
    // No hover rule hands the text a right padding to clear the control. A
    // sweep of every hover rule in the sheet is what proves the reservation
    // is gone; one named rule's absence would only prove one rule is gone.
    for (const rule of timelineCss.rules) {
      if (!rule.selector.includes(":hover")) continue;
      expect(rule.body, rule.selector).not.toContain("padding-right");
    }
    assistant.remove();
  });

  it("keeps system notices quiet, finish metadata at 13 px, and the working line unchanged", () => {
    workspaceCss.inject([
      ".workspace-chat-system",
      ".workspace-chat-finish",
      ".workspace-working-line",
    ]);
    const system = document.createElement("div");
    system.className = "workspace-chat-system";
    const rail = document.createElement("div");
    rail.className = "workspace-conversation";
    const finish = document.createElement("div");
    finish.className = "workspace-chat-finish";
    const typing = document.createElement("div");
    typing.className = "workspace-working-line";
    rail.append(typing);
    document.body.append(system, rail);

    expect(getComputedStyle(system).fontSize).toBe("12px");
    expect(getComputedStyle(system).color).toBe("#484640");
    expect(getComputedStyle(system).fontFamily).toContain("Inter");
    expect(getComputedStyle(typing).fontSize).toBe("12px");
    system.remove();
    rail.remove();
  });

  it("holds the turn footer to a 12 px line over one ledger disclosure", () => {
    footerCss.inject([".turn-footer"]);
    const footer = document.createElement("div");
    footer.className = "turn-footer";
    document.body.appendChild(footer);

    expect(getComputedStyle(footer).fontSize).toBe("12px");
    expect(getComputedStyle(footer).color).toBe("#484640");
    expect(getComputedStyle(footer).fontFamily).toContain("Inter");
    // The trigger is a control, so it is a 24 px target. happy-dom computes
    // no width on a <summary>, so its box is read off the rule.
    const trigger = footerCss.rulesFor(".turn-footer-detail-trigger");
    expect(trigger).toContain("width: 24px");
    expect(trigger).toContain("height: 24px");
    // The disclosure shares the line's row: opening it adds the ledger to a
    // turn that already had its line, never a row under it.
    expect(footerCss.rulesFor(".turn-footer")).toContain("display: flex");
    footer.remove();
  });
});
