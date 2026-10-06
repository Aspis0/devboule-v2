// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkingLine } from "./WorkingLine";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const START = 1_790_000_000_000;

let host: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(START + 72_000);
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root.unmount());
  host.remove();
  vi.useRealTimers();
});

describe("the working line", () => {
  it("says the turn is working, for how long since it began, and how to stop it", async () => {
    await act(async () => root.render(<WorkingLine startedAtMs={START} />));
    const line = host.querySelector(".workspace-working-line");
    expect(line?.textContent).toBe("Working…01:12esc to interrupt");
  });

  it("counts from the turn's start, not from when the line was drawn", async () => {
    await act(async () => root.render(<WorkingLine startedAtMs={START} />));
    // Leaving the pane and coming back draws the line anew: the turn's age is unchanged.
    await act(async () => root.render(null));
    await act(async () => root.render(<WorkingLine startedAtMs={START} />));
    expect(host.querySelector(".workspace-working-clock")?.textContent).toBe("01:12");

    await act(async () => {
      vi.advanceTimersByTime(3_000);
    });
    expect(host.querySelector(".workspace-working-clock")?.textContent).toBe("01:15");
  });

  it("draws no clock, and schedules none, for a turn whose start is not known", async () => {
    await act(async () => root.render(<WorkingLine startedAtMs={null} />));

    expect(host.querySelector(".workspace-working-clock")).toBeNull();
    expect(host.querySelector(".workspace-working-line")?.textContent).toBe(
      "Working…esc to interrupt",
    );
    expect(vi.getTimerCount()).toBe(0);
  });

  it("is one polite region whose announced words never change as the clock ticks", async () => {
    await act(async () => root.render(<WorkingLine startedAtMs={START} />));
    const line = host.querySelector(".workspace-working-line");
    expect(line?.getAttribute("role")).toBe("status");
    // The only thing that changes is hidden from assistive tech, and the dot is only a dot.
    expect(host.querySelector(".workspace-working-clock")?.getAttribute("aria-hidden")).toBe(
      "true",
    );
    expect(host.querySelector(".workspace-working-dot")?.getAttribute("aria-hidden")).toBe("true");
    expect(host.querySelector(".workspace-working-dot")?.classList.contains("dot-pulse")).toBe(
      false,
    );
    const spoken = (): string =>
      Array.from(line?.children ?? [])
        .filter((child) => child.getAttribute("aria-hidden") !== "true")
        .map((child) => child.textContent)
        .join("|");
    const before = spoken();

    await act(async () => {
      vi.advanceTimersByTime(60_000);
    });

    expect(spoken()).toBe(before);
  });

  it("stops its clock when it goes away", async () => {
    await act(async () => root.render(<WorkingLine startedAtMs={START} />));
    await act(async () => root.render(null));
    expect(vi.getTimerCount()).toBe(0);
  });
});
