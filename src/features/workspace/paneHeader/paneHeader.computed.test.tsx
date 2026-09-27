// @vitest-environment happy-dom

// The pane header's spec geometry against the real stylesheets: the assembled
// sheets in bundle order, so a restyle that loses the spec values fails here,
// not live. Order mirrors the module graph: tokens and global from main.tsx,
// paneHeader.css through the surfaces (Workspace.tsx:18-19), strip.css through
// SessionStrip (:23), Workspace.css last (Workspace.tsx:80).
//
// The inject list names the Workspace.css shared group rule bare
// (".workspace-agent-status") alongside the header's descendant selectors on
// purpose: the group loads later at equal-or-lower specificity, so if the
// header's rules are ever rewritten to bare selectors the group wins and
// these assertions go mono 10 — the test fails without the fix.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";
import { PaneHeader } from "./PaneHeader";
import { headerMenu } from "./paneHeaderMenu";
import type { HeaderDisplay } from "./paneHeaderStatus";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

afterEach(async () => {
  document.body.replaceChildren();
  removeCssProof();
});

const RUNNING: HeaderDisplay = { word: "Running", tone: "green", pulse: true, tooltip: "Running" };
const QUIET: HeaderDisplay = {
  word: "Quiet",
  tone: "border",
  pulse: false,
  tooltip: "Quiet — no output, may still be working.",
};

async function renderHeader(
  kind: "agent" | "terminal",
  display: HeaderDisplay,
): Promise<{ token: (name: string) => string | undefined }> {
  const { inject, token } = assembleCssProof([
    read("src/styles/tokens.css"),
    read("src/styles/global.css"),
    read("src/features/workspace/paneHeader/paneHeader.css"),
    read("src/features/workspace/strip/strip.css"),
    read("src/features/workspace/Workspace.css"),
  ]);
  inject([
    ".workspace-agent-toolbar",
    ".workspace-terminal-toolbar",
    ".workspace-agent-toolbar .workspace-status-dot",
    ".workspace-terminal-toolbar .workspace-status-dot",
    ".workspace-agent-toolbar .workspace-agent-title",
    ".workspace-terminal-toolbar .workspace-terminal-title",
    ".workspace-agent-toolbar .workspace-agent-status",
    ".workspace-agent-status",
    ".workspace-terminal-toolbar .workspace-terminal-status",
    ".pane-header-kebab",
  ]);
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host);
  await act(async () => {
    root.render(
      kind === "agent" ? (
        <PaneHeader
          kind="agent"
          title="Claude"
          display={display}
          menu={headerMenu("C:\\x", undefined)}
        />
      ) : (
        <PaneHeader
          kind="terminal"
          title="zsh · api"
          display={display}
          menu={headerMenu("C:\\x", undefined)}
          trailingSlot={
            <>
              <button type="button" className="workspace-terminal-interrupt">
                Ctrl+C
              </button>
              <button type="button" className="workspace-terminal-close">
                Close
              </button>
            </>
          }
        />
      ),
    );
  });
  return { token };
}

describe("pane header computed styles (real stylesheets, no app launch)", () => {
  it("is a single h36 row with the spec hairline", async () => {
    const { token } = await renderHeader("agent", RUNNING);
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
    const { token } = await renderHeader("agent", RUNNING);
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

  it("keeps the live dot at 6 and the kebab at 28, last in the row", async () => {
    await renderHeader("agent", RUNNING);
    const header = document.querySelector<HTMLElement>(".workspace-agent-toolbar");
    if (header === null) throw new Error("pane header did not render");
    const dot = header.querySelector<HTMLElement>(".workspace-status-dot");
    if (dot === null) throw new Error("pane dot did not render");
    expect(getComputedStyle(dot).width).toBe("6px");
    expect(getComputedStyle(dot).height).toBe("6px");
    const kebab = header.querySelector<HTMLElement>(".pane-header-kebab");
    if (kebab === null) throw new Error("pane kebab did not render");
    const kebabStyle = getComputedStyle(kebab);
    expect(kebabStyle.width).toBe("28px");
    expect(kebabStyle.height).toBe("28px");
    expect(header.lastElementChild).toBe(kebab);
  });

  it("pulses the dot exactly while the display says so", async () => {
    await renderHeader("agent", RUNNING);
    expect(
      document.querySelector(".workspace-agent-toolbar .workspace-status-dot")?.className,
    ).toContain("dot-pulse");
    document.body.replaceChildren();
    removeCssProof();
    await renderHeader("agent", QUIET);
    expect(
      document.querySelector(".workspace-agent-toolbar .workspace-status-dot")?.className,
    ).not.toContain("dot-pulse");
  });

  it("gives the terminal the same row, word scale and trailing order", async () => {
    const { token } = await renderHeader("terminal", QUIET);
    const header = document.querySelector<HTMLElement>(".workspace-terminal-toolbar");
    if (header === null) throw new Error("terminal header did not render");
    const style = getComputedStyle(header);
    expect(style.height).toBe("36px");
    expect(style.borderBottomWidth).toBe("1px");
    expect(style.borderBottomColor).toBe(token("--line"));
    const title = header.querySelector<HTMLElement>(".workspace-terminal-title");
    if (title === null) throw new Error("terminal title did not render");
    expect(getComputedStyle(title).fontSize).toBe("14px");
    expect(getComputedStyle(title).color).toBe(token("--ink"));
    const status = header.querySelector<HTMLElement>(".workspace-terminal-status");
    if (status === null) throw new Error("terminal status did not render");
    const statusStyle = getComputedStyle(status);
    expect(statusStyle.fontSize).toBe("12px");
    expect(statusStyle.color).toBe(token("--muted"));
    expect(statusStyle.fontFamily).not.toContain("JetBrains");
    // The status grows so the trailing actions sit at the right edge with
    // the kebab last: dot, title, status, buttons, kebab.
    expect(statusStyle.flexGrow).toBe("1");
    const order = [...header.children].map((child) => child.className);
    expect(order.at(-1)).toContain("pane-header-kebab");
    expect(order.at(-2)).toContain("workspace-terminal-close");
    expect(order.at(-3)).toContain("workspace-terminal-interrupt");
  });
});
