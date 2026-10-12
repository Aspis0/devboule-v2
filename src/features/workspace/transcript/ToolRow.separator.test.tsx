// @vitest-environment happy-dom
// The verb and its target sit side by side on every tool row, with no dot between
// them (calm-chat SPEC §a #5). The failure word keeps its own dot, where calm-chat
// #7 asks for it.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | undefined;
let root: Root | undefined;

afterEach(async () => {
  if (root !== undefined) await act(async () => root?.unmount());
  container?.remove();
  root = undefined;
  container = undefined;
});

async function render(item: ToolChatItem) {
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
  await act(async () => {
    root?.render(<ToolRow item={item} transcriptEnded={false} />);
  });
  return container;
}

const row = (overrides: Partial<ToolChatItem>): ToolChatItem => ({
  id: "tool-1",
  role: "tool",
  title: "src/lib.rs",
  output: "",
  toolCallId: "t1",
  status: "completed",
  ...overrides,
});

describe("the tool row's verb and target", () => {
  it("has no dot between an edit row's verb and its file", async () => {
    const view = await render(
      row({
        title: "kalsa-site/assets/site.css",
        kind: "edit",
        edits: [{ path: "kalsa-site/assets/site.css", kind: "edit", added: 12, removed: 30 }],
      }),
    );
    expect(view.querySelector(".workspace-chat-tool-sep")).toBeNull();
    expect(view.textContent).toContain("kalsa-site/assets/site.css");
  });

  it("has no dot between a read row's verb and its file either", async () => {
    const view = await render(row({ title: "src/lib.rs", kind: "read" }));
    expect(view.querySelector(".workspace-chat-tool-sep")).toBeNull();
  });
});
