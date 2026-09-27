// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "./cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");
const workspaceCss = assembleCssProof([
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/features/workspace/Workspace.css"), "utf8"),
]);

afterEach(removeCssProof);

describe("tool row computed styles", () => {
  it("uses the grouped tool row surface and UI typography", () => {
    workspaceCss.inject([
      ".workspace-chat-tool",
      ".workspace-chat-tool-group",
      ".workspace-chat-tool-group-summary",
      ".workspace-chat-tool-group-summary > svg",
      ".workspace-chat-tool-running",
    ]);
    const tool = document.createElement("details");
    tool.className = "workspace-chat-tool";
    document.body.appendChild(tool);
    const group = document.createElement("details");
    group.className = "workspace-chat-tool-group";
    document.body.appendChild(group);

    const style = getComputedStyle(group);
    expect(style.minHeight).toBe("24px");
    expect(style.borderRadius).toBe("6px");
    expect(style.backgroundColor).toBe("#ece4d4");
    expect(style.fontSize).toBe("12px");
    expect(style.color).toBe("#4a463e");
    expect(style.fontFamily).not.toContain("JetBrains Mono");
    expect(getComputedStyle(tool).minHeight).toBe("24px");
    expect(getComputedStyle(tool).fontFamily).not.toContain("JetBrains Mono");
    tool.remove();
    group.remove();
  });

  it("uses code colors and mono 12/1.45 typography in an open tool body", () => {
    workspaceCss.inject([
      ".workspace-chat-tool[open]",
      ".workspace-chat-tool[open] > summary",
      ".workspace-chat-tool[open] .workspace-chat-tool-summary-text",
      ".workspace-chat-tool-label",
      ".workspace-chat-tool-body",
    ]);
    const tool = document.createElement("details");
    tool.className = "workspace-chat-tool";
    tool.open = true;
    const summary = document.createElement("summary");
    const label = document.createElement("span");
    label.className = "workspace-chat-tool-label";
    const command = document.createElement("span");
    command.className = "workspace-chat-tool-summary-text";
    summary.append(label, command);
    tool.append(summary);
    const body = document.createElement("div");
    body.className = "workspace-chat-tool-body";
    tool.append(body);
    document.body.appendChild(tool);

    expect(getComputedStyle(tool).backgroundColor).toBe("#262019");
    expect(getComputedStyle(summary).color).toBe("#e9e1d3");
    expect(getComputedStyle(command).fontFamily).toContain("JetBrains Mono");
    expect(getComputedStyle(label).fontFamily).not.toContain("JetBrains Mono");
    const style = getComputedStyle(body);
    expect(style.backgroundColor).toBe("#262019");
    expect(style.color).toBe("#e9e1d3");
    expect(style.fontFamily).toContain("JetBrains Mono");
    expect(style.fontSize).toBe("12px");
    expect(style.lineHeight).toBe("1.45");
    tool.remove();
  });
});
