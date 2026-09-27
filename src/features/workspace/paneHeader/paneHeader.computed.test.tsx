// @vitest-environment happy-dom

// The pane header's spec geometry against the real stylesheets: the assembled
// sheets in bundle order, so a restyle that loses the spec values fails here,
// not live. Order mirrors the module graph: tokens and global from main.tsx,
// paneHeader.css through the surfaces (Workspace.tsx:18-19), strip.css through
// SessionStrip (:23), Workspace.css last (Workspace.tsx:80) — which is why the
// header's restyle rules use descendant selectors: a bare rule here would lose
// to Workspace.css's surviving shared groups.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";
import { PaneHeader } from "./PaneHeader";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

afterEach(async () => {
  document.body.replaceChildren();
  removeCssProof();
});

async function renderAgentHeader(): Promise<{ token: (name: string) => string | undefined }> {
  const { inject, token } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/workspace/paneHeader/paneHeader.css"),
    read("src/features/workspace/strip/strip.css"),
    read("src/features/workspace/Workspace.css"),
  ]);
  inject([
    ".workspace-agent-toolbar",
    ".workspace-agent-toolbar .workspace-status-dot",
    ".workspace-agent-toolbar .workspace-agent-title",
    ".workspace-agent-toolbar .workspace-agent-status",
    ".pane-header-kebab",
  ]);
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      <PaneHeader kind="agent" title="Claude" statusWord="Working…" dotTone="green" cwd="C:\\x" />,
    );
  });
  return { token };
}

describe("pane header computed styles (real stylesheets, no app launch)", () => {
  it("is a single h36 row with the spec hairline", async () => {
    const { token } = await renderAgentHeader();
    const header = document.querySelector<HTMLElement>(".workspace-agent-toolbar");
    if (header === null) throw new Error("pane header did not render");
    const style = getComputedStyle(header);
    expect(style.display).toBe("flex");
    expect(style.height).toBe("36px");
    expect(style.borderBottomWidth).toBe("1px");
    expect(style.borderBottomStyle).toBe("solid");
    expect(style.borderBottomColor).toBe(token("--line"));
    expect(style.flexWrap).not.toBe("wrap");
  });

  it("sets the title 14 in ink and the status word 12 muted without mono", async () => {
    const { token } = await renderAgentHeader();
    const title = document.querySelector<HTMLElement>(".workspace-agent-title");
    if (title === null) throw new Error("pane title did not render");
    expect(getComputedStyle(title).fontSize).toBe("14px");
    expect(getComputedStyle(title).color).toBe(token("--ink"));
    const status = document.querySelector<HTMLElement>(".workspace-agent-status");
    if (status === null) throw new Error("pane status did not render");
    const statusStyle = getComputedStyle(status);
    expect(statusStyle.fontSize).toBe("12px");
    expect(statusStyle.color).toBe(token("--muted"));
    expect(statusStyle.fontFamily).not.toContain("JetBrains");
  });

  it("keeps the live dot at 6 and the kebab at 28 on the right", async () => {
    await renderAgentHeader();
    const dot = document.querySelector<HTMLElement>(".workspace-status-dot");
    if (dot === null) throw new Error("pane dot did not render");
    expect(getComputedStyle(dot).width).toBe("6px");
    expect(getComputedStyle(dot).height).toBe("6px");
    const kebab = document.querySelector<HTMLElement>(".pane-header-kebab");
    if (kebab === null) throw new Error("pane kebab did not render");
    const kebabStyle = getComputedStyle(kebab);
    expect(kebabStyle.width).toBe("28px");
    expect(kebabStyle.height).toBe("28px");
    expect(kebabStyle.marginLeft).toBe("auto");
  });
});
