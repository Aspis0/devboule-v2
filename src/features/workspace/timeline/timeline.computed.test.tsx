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

afterEach(removeCssProof);

describe("timeline computed styles", () => {
  it("centers a 760 px rail with the specified padding and gaps the card from the transcript", () => {
    workspaceCss.inject([".workspace-conversation", ".workspace-conversation-content"]);
    const rail = document.createElement("div");
    rail.className = "workspace-conversation";
    const content = document.createElement("div");
    content.className = "workspace-conversation-content";
    rail.append(content);
    document.body.appendChild(rail);

    const style = getComputedStyle(rail);
    expect(style.maxWidth).toBe("760px");
    expect(style.marginLeft).toBe("auto");
    expect(style.marginRight).toBe("auto");
    expect(style.paddingTop).toBe("4px");
    expect(style.paddingRight).toBe("24px");
    expect(style.paddingBottom).toBe("0px");
    // The container's gap now only separates the transcript from the permission
    // card; the entry rhythm lives on the wrapper.
    expect(style.gap).toBe("10px");
    expect(getComputedStyle(content).gap).toBe("10px");
    rail.remove();
  });

  it("keeps the transcript wrapper unsquashed beside a tall permission card", () => {
    workspaceCss.inject([".workspace-conversation", ".workspace-conversation-content"]);
    const rail = document.createElement("div");
    rail.className = "workspace-conversation";
    const content = document.createElement("div");
    content.className = "workspace-conversation-content";
    rail.append(content);
    document.body.appendChild(rail);

    // Every content child the wrapper replaced carries `flex: none`
    // (`.workspace-chat-entry`); the wrapper is the one flex item in the column
    // that must not shrink, or a tall card compresses the transcript under it.
    expect(getComputedStyle(content).flexShrink).toBe("0");
    rail.remove();
  });

  it("gives user messages the bubble width, inset, corner shape, fill, and ink", () => {
    timelineCss.inject([".workspace-chat-bubble"]);
    const bubble = document.createElement("div");
    bubble.className = "workspace-chat-bubble";
    document.body.appendChild(bubble);

    const style = getComputedStyle(bubble);
    expect(style.maxWidth).toBe("70%");
    expect(style.padding).toBe("16px");
    expect(style.borderRadius).toBe("16px 2px 16px 16px");
    expect(style.backgroundColor).toBe("#fbf8f1");
    expect(style.color).toBe("#1c1a17");
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
      ".workspace-chat-assistant .workspace-chat-copy pre",
      ".timeline-copy-chip",
      ".timeline-copy-chip:focus-visible",
      ".workspace-chat-user:hover .timeline-copy-chip",
      ".workspace-chat-user:focus-within .timeline-copy-chip",
      ".workspace-chat-assistant:hover .timeline-copy-chip",
      ".workspace-chat-assistant:focus-within .timeline-copy-chip",
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
      '<button class="timeline-copy-chip" type="button">Copy</button>',
      "</div>",
    ].join("");
    document.body.appendChild(assistant);

    const heading = assistant.querySelector<HTMLElement>(".plan-markdown-heading-3");
    const list = assistant.querySelector<HTMLElement>("ul");
    const inlineCode = assistant.querySelector<HTMLElement>(".workspace-chat-copy > code");
    const codeBlock = assistant.querySelector<HTMLElement>("pre");
    const copy = assistant.querySelector<HTMLButtonElement>(".timeline-copy-chip");
    if (
      heading === null ||
      list === null ||
      inlineCode === null ||
      codeBlock === null ||
      copy === null
    ) {
      throw new Error("Markdown or copy markup did not render");
    }
    expect(getComputedStyle(assistant).fontSize).toBe("14px");
    expect(heading).not.toBeNull();
    for (const level of [1, 2, 3, 4, 5, 6]) {
      const levelHeading = assistant.querySelector<HTMLElement>(`.plan-markdown-heading-${level}`);
      expect(levelHeading).not.toBeNull();
      expect(getComputedStyle(levelHeading!).fontSize).toBe(
        ({ 1: "20px", 2: "16px", 3: "14px", 4: "13px", 5: "12px", 6: "12px" } as const)[
          level as 1 | 2 | 3 | 4 | 5 | 6
        ],
      );
      expect(getComputedStyle(levelHeading!).fontWeight).toBe("600");
    }
    expect(getComputedStyle(list).paddingLeft).toBe("18px");
    expect(getComputedStyle(inlineCode).fontSize).toBe("13px");
    expect(getComputedStyle(inlineCode).fontFamily).toContain("JetBrains Mono");
    expect(getComputedStyle(codeBlock).backgroundColor).toBe("#262019");
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
    assistant.remove();
  });

  it("keeps system notices quiet, finish metadata at 13 px, and typing unchanged", () => {
    workspaceCss.inject([
      ".workspace-chat-system",
      ".workspace-chat-finish",
      ".workspace-chat-typing",
    ]);
    const system = document.createElement("div");
    system.className = "workspace-chat-system";
    const rail = document.createElement("div");
    rail.className = "workspace-conversation";
    const finish = document.createElement("div");
    finish.className = "workspace-chat-finish";
    const typing = document.createElement("div");
    typing.className = "workspace-chat-typing";
    rail.append(typing);
    document.body.append(system, finish, rail);

    expect(getComputedStyle(system).fontSize).toBe("12px");
    expect(getComputedStyle(system).color).toBe("#686256");
    expect(getComputedStyle(system).fontFamily).toContain("Inter");
    expect(getComputedStyle(finish).fontSize).toBe("13px");
    expect(getComputedStyle(finish).color).toBe("#686256");
    expect(getComputedStyle(finish).fontFamily).toContain("Inter");
    expect(getComputedStyle(typing).fontSize).toBe("11px");
    system.remove();
    rail.remove();
  });
});
