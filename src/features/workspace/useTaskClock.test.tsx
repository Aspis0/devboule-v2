// @vitest-environment happy-dom
// The clock the Tasks tab measures running rows by: it reads the time when it
// starts running, and it keeps no timer while nothing runs.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useTaskClock } from "./useTaskClock";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function Probe({ live }: { live: boolean }) {
  return <span data-testid="now">{useTaskClock(live)}</span>;
}

function shown(): number {
  return Number(container.querySelector('[data-testid="now"]')?.textContent);
}

async function render(live: boolean): Promise<void> {
  await act(async () => root.render(<Probe live={live} />));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(1_000));
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(async () => {
  await act(async () => root.unmount());
  container.remove();
  vi.useRealTimers();
});

describe("the Tasks tab clock", () => {
  it("reads the time again as soon as something starts running", async () => {
    await render(false);
    vi.setSystemTime(new Date(40_000));

    await render(true);

    expect(shown()).toBe(40_000);
  });

  it("ticks once a second while live", async () => {
    await render(true);

    await act(async () => {
      vi.advanceTimersByTime(2_000);
    });

    expect(shown()).toBe(3_000);
  });

  it("keeps no timer while idle", async () => {
    await render(false);

    expect(vi.getTimerCount()).toBe(0);
  });
});
