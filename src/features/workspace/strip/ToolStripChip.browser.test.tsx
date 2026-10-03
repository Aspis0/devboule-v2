// @vitest-environment happy-dom

// What a browser tab's chip says about its page: the page's own favicon once
// there is one, a loading ring while the page is still loading, and neither on
// a tab restored after a restart — that tab has a record and no page, so
// nothing is loading and there is no icon to show.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { describe, expect, it } from "vitest";
import { localWorkspaceKey, type WorkspaceKey } from "../hosts/hostIdentity";
import { ToolStripChip, type BrowserTabPage } from "./StripChip";
import { makeBrowserTab } from "./toolTabs";

function workspace(id: string): WorkspaceKey {
  const key = localWorkspaceKey(id);
  if (key === null) throw new Error(`not a workspace key: ${id}`);
  return key;
}

const WORKSPACE = workspace("alpha");

function page(over: Partial<BrowserTabPage> = {}): BrowserTabPage {
  return {
    label: "Example Domain",
    favicon: null,
    url: "https://example.com/",
    loading: false,
    ...over,
  };
}

async function renderChip(browser?: BrowserTabPage): Promise<HTMLElement> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(
      <ToolStripChip
        tool={makeBrowserTab(WORKSPACE, "tab-1")}
        selected={false}
        multiselected={false}
        tabIndex={0}
        browser={browser}
        tooltip="https://example.com/"
        menuOpen={false}
        onTabClick={() => undefined}
        onTabAuxClick={() => undefined}
        onRowContextMenu={() => undefined}
        onChipKeyDown={() => undefined}
        onClose={() => undefined}
      />,
    );
  });
  return container;
}

describe("a browser tab's chip", () => {
  it("shows a globe while the page has declared no icon", async () => {
    const container = await renderChip(page());
    expect(container.querySelector(".strip-browser-favicon")).toBeNull();
    expect(container.querySelector(".strip-browser-loading")).toBeNull();
    expect(container.querySelector("svg")).not.toBeNull();
  });

  it("shows the page's own favicon once it has one", async () => {
    const container = await renderChip(page({ favicon: "https://example.com/icon.png" }));
    expect(container.querySelector("img")?.getAttribute("src")).toBe(
      "https://example.com/icon.png",
    );
  });

  it("shows a loading ring in the favicon's place while the page loads", async () => {
    const container = await renderChip(page({ loading: true }));
    expect(container.querySelector(".strip-browser-loading")).not.toBeNull();
    // The ring replaces the icon rather than joining it, so the chip does not
    // grow a column while the page arrives.
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(".workspace-sr-only")?.textContent).toContain("loading");
  });

  it("shows no ring for a restored tab, which has a record and no page", async () => {
    const container = await renderChip();
    expect(container.querySelector(".strip-browser-loading")).toBeNull();
    expect(container.querySelector(".workspace-sr-only")?.textContent).not.toContain("loading");
  });
});
