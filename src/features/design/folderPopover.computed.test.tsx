// @vitest-environment happy-dom

// The folder popover's contract with a directory path too wide for it: the
// path line ellipsizes on one line with the full path in its title, the
// shell child that carries the line may shrink below the path's width, its
// own option list keeps its full height under the shell's cap, every other
// line inherits overflow-wrap anywhere from the shell, and the popover body
// declares overflow-x hidden beside its overflow-y auto scroll. Every style
// is read as computed from the injected real sheets, so a declaration nobody
// wrote reads as undeclared.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import { collectSrcSheets } from "../../styles/srcSheets";
import { assembleCssProof, removeCssProof } from "../workspace/cssProof";
import { DesignFolderControl } from "./DesignFolderControl";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const proof = assembleCssProof(collectSrcSheets().map((sheet) => sheet.css));

const FOLDER_PATH =
  "C:\\Users\\gualt\\Desktop\\New devboule\\devboule-v2-c17\\an\\example\\checkout\\long\\enough\\to\\overflow";

let mounted: { root: ReturnType<typeof createRoot>; container: HTMLDivElement } | null = null;

afterEach(async () => {
  removeCssProof();
  if (mounted === null) return;
  const current = mounted;
  mounted = null;
  await act(async () => current.root.unmount());
  current.container.remove();
});

async function openPopover(selectionNotice: string | null = null): Promise<HTMLDivElement> {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);
  mounted = { root, container };
  await act(async () => {
    root.render(
      <DesignFolderControl
        folders={[
          {
            id: "folder-1",
            name: "devboule-v2-c17",
            path: FOLDER_PATH,
            workspaces: [
              {
                id: "ws-1",
                projectId: "folder-1",
                title: "main",
                isolation: "local",
                path: FOLDER_PATH,
              },
            ],
          },
        ]}
        loading={false}
        refreshing={false}
        foldersError={null}
        selectionNotice={selectionNotice}
        selectedWorkspaceId={null}
        selectionUnresolved={false}
        attachedPath={null}
        disabled={false}
        attachBusy={false}
        attachError={null}
        onOpen={() => undefined}
        onSelect={() => undefined}
        onAttach={async () => false}
        onUseFolder={async () => false}
      />,
    );
  });
  const trigger = container.querySelector<HTMLButtonElement>('[data-design-folder-trigger="true"]');
  if (trigger === null) throw new Error("folder trigger did not render");
  await act(async () => trigger.click());
  const picker = container.querySelector<HTMLDivElement>("#design-folder-picker");
  if (picker === null) throw new Error("folder popover did not open");
  return picker;
}

describe("the Design folder popover over a path wider than it", () => {
  it("ellipsizes the path line on one line and keeps the full path in title", async () => {
    const picker = await openPopover();
    const path = picker.querySelector<HTMLElement>(".design-folder-path");
    if (path === null) throw new Error("path line did not render");
    proof.inject([".design-folder-path"]);
    const style = getComputedStyle(path);
    expect(style.textOverflow).toBe("ellipsis");
    expect(style.whiteSpace).toBe("nowrap");
    expect(style.overflow).toBe("hidden");
    expect(path.getAttribute("title")).toBe(FOLDER_PATH);
  });

  it("keeps the folder record shrinkable as the popover's flex child", async () => {
    const picker = await openPopover();
    const record = picker.querySelector<HTMLElement>(".design-folder-record");
    if (record === null) throw new Error("folder record did not render");
    proof.inject([".design-agent-picker", ".design-folder-record"]);
    expect(getComputedStyle(picker).display).toBe("flex");
    expect(["0", "0px"]).toContain(getComputedStyle(record).minWidth);
  });

  it("keeps its own option list at full height: the list never shrinks", async () => {
    const picker = await openPopover();
    const list = [...picker.children].find((child) =>
      child.classList.contains("design-agent-picker-options"),
    );
    if (list === undefined) throw new Error("the folder's own option list did not render");
    proof.inject([".design-folder-picker > .design-agent-picker-options"]);
    // The list is a scroll container, so its automatic minimum is zero;
    // flex: none is the pairing that keeps the shell's cap from squeezing
    // the "Don't attach a folder" row to its padding. Computed, the shorthand
    // serializes as its resolved form: 0 0 auto.
    expect(getComputedStyle(list).flex).toBe("0 0 auto");
  });

  it("declares the popover body clipped in x, scrollable in y, broken anywhere", async () => {
    const picker = await openPopover();
    proof.inject([".design-agent-picker", ".design-folder-picker"]);
    const style = getComputedStyle(picker);
    // happy-dom pairs no axes: "visible" would read green here while a browser computes it back to auto.
    expect(["hidden", "clip"]).toContain(style.overflowX);
    expect(style.overflowY).toBe("auto");
    expect(style.overflowWrap).toBe("anywhere");
  });

  it("declares no nowrap on the label, status or option lines", async () => {
    const picker = await openPopover("x".repeat(60));
    proof.inject([
      ".design-agent-picker-label",
      ".design-agent-picker-status",
      ".design-agent-picker-option",
    ]);
    const lines = [
      ".design-agent-picker-label",
      ".design-agent-picker-status",
      ".design-agent-picker-option",
    ].map((selector) => {
      const line = picker.querySelector<HTMLElement>(selector);
      if (line === null) throw new Error(`${selector} did not render`);
      return line;
    });
    for (const line of lines) {
      expect(getComputedStyle(line).whiteSpace).not.toBe("nowrap");
    }
  });
});
