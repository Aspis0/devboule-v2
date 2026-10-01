// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useMenuCopyFeedback } from "./useMenuCopyFeedback";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const writeText = vi.fn(async (): Promise<void> => undefined);
let host: HTMLDivElement;
let root: Root | null;

function CopyMenu() {
  const feedback = useMenuCopyFeedback();
  return (
    <>
      <button onClick={() => void feedback.copy("copy-path", "C:/p", "Path")}>
        {feedback.labelFor("copy-path", "Copy path")}
      </button>
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

describe("menu copy feedback lifetime", () => {
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
});
