// @vitest-environment happy-dom

// A failed call says why on its closed line, so the person need not open it.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { ToolRow } from "./ToolRow";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const SANDBOX_REFUSAL =
  "Failed to create unified exec process: CreateProcessAsUserW failed: 5 (Access is denied.)";

function tool(overrides: Partial<ToolChatItem> = {}): ToolChatItem {
  return {
    id: "tool-1",
    role: "tool",
    title: "Ran command",
    output: "",
    toolCallId: "t1",
    status: "completed",
    kind: "execute",
    ...overrides,
  };
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement | null = null;

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host?.remove();
  host = null;
});

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  return host;
}

function summaryTexts(container: HTMLElement): string[] {
  return Array.from(container.querySelectorAll(".workspace-chat-tool-summary-text")).map(
    (element) => element.textContent ?? "",
  );
}

describe("ToolRow failure reason", () => {
  it("names the reason on the closed line of a failed call, with the full text as its title", async () => {
    const container = await renderRow(
      tool({
        status: "failed",
        output: `${SANDBOX_REFUSAL}\ncwd=C:\\Users\\gualt\\Desktop\\test`,
      }),
    );

    expect(summaryTexts(container)).toContain(SANDBOX_REFUSAL);
    const reason = Array.from(
      container.querySelectorAll<HTMLElement>(".workspace-chat-tool-summary-text"),
    ).find((element) => element.textContent === SANDBOX_REFUSAL);
    expect(reason?.getAttribute("title")).toBe(SANDBOX_REFUSAL);
  });

  it("names no reason on a call that went well", async () => {
    const container = await renderRow(tool({ status: "completed", output: "ran fine" }));

    expect(summaryTexts(container)).not.toContain("ran fine");
  });
});
