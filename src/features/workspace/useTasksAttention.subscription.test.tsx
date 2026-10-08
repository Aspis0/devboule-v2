// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import { subscribeTasksNews } from "../../lib/agentSessionRegistry";
import { fakeTaskSource } from "./backgroundTaskSourceHarness";
import { useTasksAttention } from "./useTasksAttention";

vi.mock("../../lib/agentSessionRegistry", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/agentSessionRegistry")>();
  return { ...actual, subscribeTasksNews: vi.fn(actual.subscribeTasksNews) };
});

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const source = fakeTaskSource(null);
function Probe({ count }: { count: number }) {
  const news = useTasksAttention({ sessionId: "fake", source }, false);
  return (
    <span>
      {count}: {String(news)}
    </span>
  );
}
afterEach(() => vi.clearAllMocks());

it("keeps one external-store subscription across unrelated renders", async () => {
  const container = document.createElement("div");
  const root = createRoot(container);
  try {
    await act(async () => root.render(<Probe count={1} />));
    await act(async () => root.render(<Probe count={2} />));
    await act(async () => root.render(<Probe count={3} />));
    expect(subscribeTasksNews).toHaveBeenCalledTimes(1);
  } finally {
    await act(async () => root.unmount());
  }
});
