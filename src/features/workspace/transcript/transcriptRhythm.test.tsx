// The transcript's vertical rhythm, proved through the real sheet: every gap
// is the lower row's top margin — 16 at a turn boundary, 12 on a standard
// assistant row, 4 in turn, 0 on a compact assistant edge.
// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");
const read = (path: string) => readFileSync(resolve(rootDir, path), "utf8");
const workspaceCss = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/features/workspace/Workspace.css"),
]);

afterEach(removeCssProof);

const RHYTHM_SELECTORS = [
  ".workspace-conversation-content",
  ".workspace-chat-entry",
  ".workspace-chat-assistant",
  ".workspace-chat-tool + .workspace-chat-assistant",
  ".workspace-chat-tool-group + .workspace-chat-assistant",
  ".workspace-chat-thought + .workspace-chat-assistant",
  ".workspace-conversation-content > :first-child.workspace-chat-entry",
  ".workspace-conversation-content > nav + .workspace-chat-entry",
  ".workspace-chat-user",
  ".workspace-chat-user + .workspace-chat-entry",
  ".workspace-chat-user + .workspace-chat-user",
  ".workspace-chat-assistant + .workspace-chat-assistant",
  ".workspace-chat-typing",
];

function row(tag: string, className: string): HTMLElement {
  const element = document.createElement(tag);
  element.className = className;
  element.textContent = "row";
  return element;
}

function margins(host: HTMLElement): string[] {
  return [...host.querySelectorAll(":scope > div, :scope > details")].map(
    (element) => getComputedStyle(element as HTMLElement).marginTop,
  );
}

describe("transcript rhythm", () => {
  it("spaces one turn 16 / 12 / 4 with a compact 0 between assistant rows", () => {
    workspaceCss.inject(RHYTHM_SELECTORS);
    const content = document.createElement("div");
    content.className = "workspace-conversation-content";
    content.append(
      row("div", "workspace-chat-entry workspace-chat-user"),
      row("div", "workspace-chat-entry workspace-chat-assistant"),
      row("details", "workspace-chat-entry workspace-chat-tool"),
      row("div", "workspace-chat-entry workspace-chat-assistant"),
      row("div", "workspace-chat-entry workspace-chat-assistant"),
      row("div", "workspace-chat-entry workspace-chat-user"),
      row("div", "workspace-chat-entry workspace-chat-user"),
      row("div", "workspace-chat-entry workspace-chat-assistant"),
      row("div", "workspace-chat-entry workspace-chat-system"),
      row("div", "workspace-chat-entry workspace-chat-assistant"),
    );
    document.body.appendChild(content);

    // first 0; assistant after user 16; tool in turn 4; assistant after
    // tool 4; assistant after assistant 0; user after assistant 16; user
    // after user 4; assistant after user 16; system in turn 4; assistant
    // after a notice back on the standard 12.
    expect(margins(content)).toEqual([
      "0px",
      "16px",
      "4px",
      "4px",
      "0px",
      "16px",
      "4px",
      "16px",
      "4px",
      "12px",
    ]);
    content.remove();
  });

  it("starts the first row at 0 with the rail on and off", () => {
    workspaceCss.inject(RHYTHM_SELECTORS);
    const withoutRail = document.createElement("div");
    withoutRail.className = "workspace-conversation-content";
    withoutRail.append(row("div", "workspace-chat-entry workspace-chat-user"));
    document.body.appendChild(withoutRail);
    expect(margins(withoutRail)).toEqual(["0px"]);
    withoutRail.remove();

    const withRail = document.createElement("div");
    withRail.className = "workspace-conversation-content";
    const rail = document.createElement("nav");
    rail.className = "turn-rail";
    withRail.append(rail, row("div", "workspace-chat-entry workspace-chat-user"));
    document.body.appendChild(withRail);
    expect(margins(withRail)).toEqual(["0px"]);
    withRail.remove();
  });

  it("glues the typing indicator to its turn", () => {
    workspaceCss.inject([...RHYTHM_SELECTORS, ".workspace-chat-typing"]);
    const content = document.createElement("div");
    content.className = "workspace-conversation-content";
    const typing = document.createElement("div");
    typing.className = "workspace-chat-typing";
    typing.textContent = "Agent is working";
    content.append(row("div", "workspace-chat-entry workspace-chat-assistant"), typing);
    document.body.appendChild(content);

    expect(getComputedStyle(typing).marginTop).toBe("4px");
    content.remove();
  });
});
