// @vitest-environment happy-dom

// A row re-renders only when its own entry changes. A neighbour's update must
// leave it alone, or every streamed token walks the whole transcript.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentChatItem } from "../../../lib/agentSession";
import { entryFrame } from "./entryFrame";
import { TranscriptRows } from "./TranscriptRows";

// entryFrame runs once per render of a row's memo boundary and once more in
// ToolRow, so its call count is a render count for the row.
vi.mock("./entryFrame", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./entryFrame")>();
  return { ...actual, entryFrame: vi.fn(actual.entryFrame) };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const names = { sessionById: new Map(), deviceNames: new Map() };

function tool(id: string, output: string): AgentChatItem {
  return {
    id,
    role: "tool",
    title: "search",
    output,
    toolCallId: id,
    status: "completed",
    kind: "search",
  };
}

function assistant(id: string, text: string): AgentChatItem {
  return { id, role: "assistant", text, messageId: null };
}

function Transcript({ items }: { items: AgentChatItem[] }) {
  return (
    <div>
      <TranscriptRows
        items={items}
        recoveredAttach={false}
        pendingPlanToolCallId={null}
        a2aNames={names}
        fileLinks={null}
        transcriptEnded={false}
        streamingThoughtId={null}
      />
    </div>
  );
}

let root: Root | null = null;
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
  vi.mocked(entryFrame).mockClear();
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  host.remove();
});

function rendersOf(id: string): number {
  return vi.mocked(entryFrame).mock.calls.filter(([item]) => item.id === id).length;
}

async function show(items: AgentChatItem[]): Promise<void> {
  await act(async () => root?.render(<Transcript items={items} />));
}

describe("transcript row renders", () => {
  it("leaves a tool row alone when a neighbouring message updates", async () => {
    const first = tool("tool-1", "one");
    await show([first, assistant("msg-1", "hello")]);
    const before = rendersOf("tool-1");
    expect(before).toBeGreaterThan(0);

    await show([first, assistant("msg-1", "hello again")]);

    expect(host.textContent).toContain("hello again");
    expect(rendersOf("tool-1")).toBe(before);
  });

  it("re-renders the tool row whose own output changed", async () => {
    const first = tool("tool-1", "one");
    await show([first, assistant("msg-1", "hello")]);
    const before = rendersOf("tool-1");

    await show([tool("tool-1", "one\ntwo"), assistant("msg-1", "hello")]);

    expect(rendersOf("tool-1")).toBeGreaterThan(before);
  });

  it("leaves a grouped row alone when another call joins its group", async () => {
    // Two consecutive calls form a group; the second is counted, since the
    // group itself reads the first call's frame.
    const first = tool("tool-1", "one");
    const second = tool("tool-2", "two");
    await show([first, second]);
    const before = rendersOf("tool-2");
    expect(before).toBeGreaterThan(0);

    await show([first, second, tool("tool-3", "three")]);

    expect(host.textContent).toContain("three");
    expect(rendersOf("tool-2")).toBe(before);
  });
});
