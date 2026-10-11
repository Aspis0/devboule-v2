// @vitest-environment happy-dom

// A closed line does not split or scan its output: a streamed result grows with
// every update, and only a line the person opened needs it.

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ToolChatItem } from "../../../lib/toolCallGroups";
import { hideUntrustedFrame } from "../../../lib/untrustedFrame";
import { ToolRow } from "./ToolRow";
import { outputLines, REASON_HEAD_CHARS } from "./toolOutputView";

vi.mock("../../../lib/untrustedFrame", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../../lib/untrustedFrame")>();
  return { ...actual, hideUntrustedFrame: vi.fn(actual.hideUntrustedFrame) };
});

vi.mock("./toolOutputView", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./toolOutputView")>();
  return { ...actual, outputLines: vi.fn(actual.outputLines) };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const BIG = Array.from({ length: 5000 }, (_, index) => `line ${index + 1}`).join("\n");

function tool(overrides: Partial<ToolChatItem> = {}): ToolChatItem {
  return {
    id: "tool-1",
    role: "tool",
    title: "q",
    output: BIG,
    toolCallId: "t1",
    status: "completed",
    kind: "search",
    ...overrides,
  };
}

let root: ReturnType<typeof createRoot> | null = null;
let host: HTMLDivElement;

beforeEach(() => {
  vi.mocked(outputLines).mockClear();
  vi.mocked(hideUntrustedFrame).mockClear();
});

afterEach(async () => {
  if (root !== null) await act(async () => root?.unmount());
  root = null;
  host.remove();
});

async function renderRow(item: ToolChatItem): Promise<HTMLElement> {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  await act(async () => root?.render(<ToolRow item={item} transcriptEnded={false} />));
  return host;
}

describe("ToolRow closed output", () => {
  it("neither frames nor splits the output of a line the person has not opened", async () => {
    await renderRow(tool());
    expect(hideUntrustedFrame).not.toHaveBeenCalled();
    expect(outputLines).not.toHaveBeenCalled();
  });

  it("works on the output once the person opens the line", async () => {
    const container = await renderRow(tool());
    const details = container.querySelector("details");
    if (details === null) throw new Error("the row had nothing to open");
    await act(async () => {
      details.open = true;
      details.dispatchEvent(new Event("toggle"));
    });
    expect(outputLines).toHaveBeenCalled();
  });

  it("reads only the head of a failure's output while the line is closed", async () => {
    const container = await renderRow(tool({ status: "failed", output: BIG }));
    const closedReads = vi.mocked(outputLines).mock.calls.map(([text]) => text.length);
    expect(closedReads.length).toBeGreaterThan(0);
    expect(Math.max(...closedReads)).toBeLessThanOrEqual(REASON_HEAD_CHARS);
    const details = container.querySelector("details");
    if (details === null) throw new Error("the failed row had nothing to open");
    await act(async () => {
      details.open = true;
      details.dispatchEvent(new Event("toggle"));
    });
    expect(vi.mocked(outputLines).mock.calls.some(([text]) => text.length === BIG.length)).toBe(
      true,
    );
  });

  it("does not read a successful browser call's output at all", async () => {
    await renderRow(tool({ kind: "browser", title: "navigate https://x.test", output: BIG }));
    expect(outputLines).not.toHaveBeenCalled();
  });
});
