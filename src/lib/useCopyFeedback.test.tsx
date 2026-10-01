// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useCopyFeedback } from "./useCopyFeedback";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const writeText = vi.fn(async (): Promise<void> => undefined);
let host: HTMLDivElement;
let root: Root | null;

function CopyMenu({
  resetAfterMs = 1500,
}: {
  resetAfterMs?: number | null | ((outcome: "copied" | "failed") => number | null);
}) {
  const feedback = useCopyFeedback({ resetAfterMs, clearOnCopy: true });
  return (
    <>
      <button onClick={() => void feedback.copy("copy-path", "C:/p", "Path")}>
        {feedback.labelFor("copy-path", "Copy path")}
      </button>
      <button onClick={feedback.reset}>Reset</button>
      <span role="status">{feedback.announcement}</span>
    </>
  );
}

beforeEach(async () => {
  vi.useFakeTimers();
  writeText.mockReset().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
  await act(async () => root!.render(<CopyMenu />));
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  host.remove();
  vi.useRealTimers();
  Reflect.deleteProperty(navigator, "clipboard");
});

describe("copy feedback lifetime", () => {
  it("ignores a clipboard completion after unmount without scheduling feedback reset", async () => {
    let complete: (() => void) | undefined;
    writeText.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          complete = resolve;
        }),
    );
    await act(async () => host.querySelector("button")!.click());
    expect(writeText).toHaveBeenCalledExactlyOnceWith("C:/p");
    expect(host.querySelector('[role="status"]')?.textContent).toBe("");
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => root!.unmount());
    root = null;
    await act(async () => complete!());
    expect(host.childElementCount).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("clears an active feedback reset timer on unmount", async () => {
    await act(async () => host.querySelector("button")!.click());
    expect(host.querySelector('[role="status"]')?.textContent).toBe("Path copied");
    expect(vi.getTimerCount()).toBe(1);
    await act(async () => root!.unmount());
    root = null;
    expect(vi.getTimerCount()).toBe(0);
  });
  it("keeps the newest result when writes finish in reverse order", async () => {
    let complete!: () => void;
    writeText.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          complete = resolve;
        }),
    );
    await act(async () => host.querySelector("button")!.click());
    writeText.mockRejectedValueOnce(new Error("denied"));
    await act(async () => host.querySelector("button")!.click());
    await act(async () => complete());
    expect(host.querySelector("button")?.textContent).toBe("Copy failed");
    expect(host.querySelector('[role="status"]')?.textContent).toBe("Path copy failed");
    expect(vi.getTimerCount()).toBe(1);
  });

  it("gives a rapid repeat copy a full feedback window", async () => {
    await act(async () => host.querySelector("button")!.click());
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    await act(async () => host.querySelector("button")!.click());
    expect(vi.getTimerCount()).toBe(1);
    await act(async () => vi.advanceTimersByTimeAsync(500));
    expect(host.querySelector("button")?.textContent).toBe("Copied");
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(host.querySelector("button")?.textContent).toBe("Copy path");
  });

  it("keeps feedback without a reset timer in persistent mode", async () => {
    await act(async () => root!.render(<CopyMenu resetAfterMs={null} />));
    await act(async () => host.querySelector("button")!.click());
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => vi.advanceTimersByTimeAsync(10000));
    expect(host.querySelector("button")?.textContent).toBe("Copied");
  });

  it.each(["rejected", "unavailable"])(
    "reports %s clipboard failure and resets it",
    async (kind) => {
      if (kind === "rejected") writeText.mockRejectedValueOnce(new Error("denied"));
      else Object.defineProperty(navigator, "clipboard", { value: undefined, configurable: true });
      await act(async () => host.querySelector("button")!.click());
      expect(host.querySelector("button")?.textContent).toBe("Copy failed");
      expect(host.querySelector('[role="status"]')?.textContent).toBe("Path copy failed");
      await act(async () => vi.advanceTimersByTimeAsync(1500));
      expect(host.querySelector("button")?.textContent).toBe("Copy path");
    },
  );
  it("can retain failure while timing success", async () => {
    await act(async () =>
      root!.render(<CopyMenu resetAfterMs={(outcome) => (outcome === "copied" ? 2000 : null)} />),
    );
    writeText.mockRejectedValueOnce(new Error("denied"));
    await act(async () => host.querySelector("button")!.click());
    expect(vi.getTimerCount()).toBe(0);
    await act(async () => vi.advanceTimersByTimeAsync(5000));
    expect(host.querySelector("button")?.textContent).toBe("Copy failed");
    await act(async () => host.querySelector("button")!.click());
    expect(vi.getTimerCount()).toBe(1);
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(host.querySelector("button")?.textContent).toBe("Copy path");
  });

  it("reset invalidates a pending copy when a persistent menu closes", async () => {
    await act(async () => root!.render(<CopyMenu resetAfterMs={null} />));
    let complete!: () => void;
    writeText.mockImplementationOnce(
      () =>
        new Promise<void>((resolve) => {
          complete = resolve;
        }),
    );
    await act(async () => host.querySelector("button")!.click());
    await act(async () => host.querySelectorAll("button")[1].click());
    await act(async () => complete());
    expect(host.querySelector("button")?.textContent).toBe("Copy path");
    expect(host.querySelector('[role="status"]')?.textContent).toBe("");
    expect(vi.getTimerCount()).toBe(0);
  });
});
