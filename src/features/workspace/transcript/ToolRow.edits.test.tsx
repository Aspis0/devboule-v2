// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function editRow(overrides: Partial<ToolChatItem> = {}): ToolChatItem {
  return {
    id: "tool-1",
    role: "tool",
    title: "kalsa-site/assets/site.css",
    output: "",
    toolCallId: "t1",
    status: "completed",
    kind: "edit",
    edits: [{ path: "kalsa-site/assets/site.css", kind: "edit", added: 12, removed: 30 }],
    ...overrides,
  };
}

let container: HTMLDivElement;
let root: Root | undefined;

afterEach(async () => {
  if (root !== undefined) await act(async () => root?.unmount());
  container?.remove();
  root = undefined;
});

async function render(item: ToolChatItem, onRevealChange?: (path: string) => void) {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => {
    root?.render(<ToolRow item={item} transcriptEnded={false} onRevealChange={onRevealChange} />);
  });
}

describe("an edit row in the transcript", () => {
  it("draws its counts after the file it changed, from the call's own edits", async () => {
    await render(editRow());
    expect(container.querySelector(".workspace-changes-counts")?.textContent).toBe("+12−30");
  });

  it("draws no counts for a call that carried no edit", async () => {
    await render(editRow({ edits: undefined }));
    expect(container.querySelector(".workspace-changes-counts")).toBeNull();
  });

  it("draws no counts while the edit runs", async () => {
    await render(editRow({ status: "running" }));
    expect(container.querySelector(".workspace-changes-counts")).toBeNull();
  });

  it("reveals the edited file in the Changes panel on click, with no inline diff", async () => {
    const onRevealChange = vi.fn();
    await render(editRow(), onRevealChange);
    const summary = container.querySelector<HTMLButtonElement>(
      "button.workspace-chat-tool-summary",
    );
    expect(summary).not.toBeNull();
    await act(async () => summary?.click());

    expect(onRevealChange).toHaveBeenCalledWith("kalsa-site/assets/site.css");
    expect(container.querySelector(".workspace-chat-tool-body")).toBeNull();
  });
});

describe("a row whose files were created", () => {
  const threeAdds: Partial<ToolChatItem> = {
    title: "3 files",
    kind: "write",
    locations: [{ path: "alpha.md" }, { path: "beta.md" }, { path: "gamma.md" }],
    edits: [
      { path: "alpha.md", kind: "write", added: 1, removed: 0 },
      { path: "beta.md", kind: "write", added: 1, removed: 0 },
      { path: "gamma.md", kind: "write", added: 1, removed: 0 },
    ],
  };

  it("reads as a creation and keeps the count of the three it made", async () => {
    await render(editRow(threeAdds));
    const label = container.querySelector(".workspace-chat-tool-label")?.textContent;
    const summary = container.querySelector(".workspace-chat-tool-summary-text")?.textContent;
    expect(`${label} ${summary}`).toBe("Create 3 files");
    expect(container.querySelector(".workspace-changes-counts")?.textContent).toBe("+3");
  });

  it("reveals the first of the three it created in the Changes panel", async () => {
    const onRevealChange = vi.fn();
    await render(editRow(threeAdds), onRevealChange);
    const summary = container.querySelector<HTMLButtonElement>(
      "button.workspace-chat-tool-summary",
    );
    expect(summary).not.toBeNull();
    await act(async () => summary?.click());

    expect(onRevealChange).toHaveBeenCalledWith("alpha.md");
  });
});
