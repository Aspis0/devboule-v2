// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { WorkingLine } from "./WorkingLine";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let host: HTMLDivElement;
let root: ReturnType<typeof createRoot>;

beforeEach(() => {
  vi.useFakeTimers();
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
  it("says the turn is working, how long, and how to stop it", async () => {
    await act(async () => root.render(<WorkingLine />));
    const line = host.querySelector(".workspace-working-line");
    expect(line?.textContent).toBe("Working…00:00esc to interrupt");
  });

  it("is one polite region whose announced words never change as the clock ticks", async () => {
    await act(async () => root.render(<WorkingLine />));
    const line = host.querySelector(".workspace-working-line");
    expect(line?.getAttribute("role")).toBe("status");
    const clock = host.querySelector(".workspace-working-clock");
    // The only thing that changes is hidden from assistive tech.
    expect(clock?.getAttribute("aria-hidden")).toBe("true");
    expect(host.querySelector(".workspace-working-dot")?.getAttribute("aria-hidden")).toBe("true");
    const spoken = (): string =>
      Array.from(line?.children ?? [])
        .filter((child) => child.getAttribute("aria-hidden") !== "true")
        .map((child) => child.textContent)
        .join("|");
    const before = spoken();

    await act(async () => {
      vi.advanceTimersByTime(72_000);
    });

    expect(clock?.textContent).toBe("01:12");
    expect(spoken()).toBe(before);
  });

  it("stops its clock when it goes away", async () => {
    await act(async () => root.render(<WorkingLine />));
    await act(async () => root.render(null));
    expect(vi.getTimerCount()).toBe(0);
  });
});
