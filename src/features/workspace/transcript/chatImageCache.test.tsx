// The shared chat image cache, seen through the thumbnail row: one read per
// reference, reads only near the viewport, released URLs revoked past the
// idle limit, and a read that never settles cut off for good.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../../lib/tauri";

const readMocks = vi.hoisted(() => ({ sessionAttachmentRead: vi.fn() }));

vi.mock("../../../lib/tauri", () => ({
  sessionAttachmentRead: readMocks.sessionAttachmentRead,
}));

import { IDLE_URL_LIMIT, READ_TIMEOUT_MS, resetChatImageCacheForTests } from "./chatImageCache";
import { ChatImageThumbnails } from "./ChatImageThumbnails";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function ref(index: number): AttachmentReference {
  return {
    sessionId: "s.owner.cache",
    digest: index.toString(16).padStart(64, "0"),
    storedBytes: 2,
  };
}

/** An observer the test drives: nothing intersects until `intersectAll`. */
class FakeIntersectionObserver {
  static instances: FakeIntersectionObserver[] = [];
  readonly targets = new Set<Element>();
  constructor(private readonly callback: IntersectionObserverCallback) {
    FakeIntersectionObserver.instances.push(this);
  }
  observe(target: Element) {
    this.targets.add(target);
  }
  unobserve(target: Element) {
    this.targets.delete(target);
  }
  disconnect() {
    this.targets.clear();
  }
  takeRecords() {
    return [];
  }
  static intersectAll() {
    for (const observer of FakeIntersectionObserver.instances) {
      const entries = [...observer.targets].map(
        (target) => ({ target, isIntersecting: true }) as IntersectionObserverEntry,
      );
      if (entries.length > 0) {
        observer.callback(entries, observer as unknown as IntersectionObserver);
      }
    }
  }
}

let container: HTMLDivElement;
let root: Root | null = null;

async function render(images: readonly AttachmentReference[][]) {
  root = createRoot(container);
  await act(async () => {
    root!.render(
      <>
        {images.map((row, index) => (
          <ChatImageThumbnails key={index} images={row} />
        ))}
      </>,
    );
  });
  await act(async () => {});
}

async function unmount() {
  await act(async () => root?.unmount());
  root = null;
}

function imageSources(): (string | null)[] {
  return Array.from(container.querySelectorAll("img")).map((img) => img.getAttribute("src"));
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  readMocks.sessionAttachmentRead.mockImplementation(async () => ({
    mimeType: "image/png",
    data: "aGk=",
  }));
  vi.stubGlobal("IntersectionObserver", undefined);
});

afterEach(async () => {
  await unmount();
  container.remove();
  resetChatImageCacheForTests();
  FakeIntersectionObserver.instances = [];
  vi.unstubAllGlobals();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});

describe("chat image cache", () => {
  it("reads a reference once for two thumbnails that show it", async () => {
    await render([[ref(1)], [ref(1)]]);
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(1);
    const sources = imageSources();
    expect(sources).toHaveLength(2);
    expect(sources[0]).toBe(sources[1]);
  });

  it("serves a viewer's sibling page from its thumbnail's read", async () => {
    await render([[ref(4), ref(5)]]);
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(2);
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label^="Attached image 1"]')?.click();
    });
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    });
    await act(async () => {});
    const page = document.querySelector('[role="dialog"] img');
    expect(page?.getAttribute("src")?.startsWith("blob:")).toBe(true);
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(2);
  });

  it("does not read an off-screen thumbnail until it nears the viewport", async () => {
    vi.stubGlobal("IntersectionObserver", FakeIntersectionObserver);
    await render([[ref(2)]]);
    expect(readMocks.sessionAttachmentRead).not.toHaveBeenCalled();
    await act(async () => FakeIntersectionObserver.intersectAll());
    await act(async () => {});
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(1);
    expect(imageSources()[0]?.startsWith("blob:")).toBe(true);
  });

  it("revokes a released URL once the idle limit is passed, never one on screen", async () => {
    let next = 0;
    vi.spyOn(URL, "createObjectURL").mockImplementation(() => `blob:test-${next++}`);
    const revoke = vi.spyOn(URL, "revokeObjectURL").mockImplementation(() => {});
    const refs = Array.from({ length: IDLE_URL_LIMIT + 1 }, (_, index) => ref(100 + index));
    await render([refs]);
    const sources = imageSources();
    expect(sources).toHaveLength(IDLE_URL_LIMIT + 1);
    expect(revoke).not.toHaveBeenCalled();
    await unmount();
    expect(revoke).toHaveBeenCalledWith(sources[0]);
    expect(revoke).not.toHaveBeenCalledWith(sources[IDLE_URL_LIMIT]);
  });

  it("drops a read that times out, so its late result never fills the entry", async () => {
    vi.useFakeTimers();
    let resolveLate!: (value: { mimeType: string; data: string }) => void;
    readMocks.sessionAttachmentRead.mockImplementationOnce(
      () => new Promise((resolve) => (resolveLate = resolve)),
    );
    const create = vi.spyOn(URL, "createObjectURL");
    await render([[ref(3)]]);
    expect(container.textContent).not.toContain("Image unavailable");
    await act(async () => {
      vi.advanceTimersByTime(READ_TIMEOUT_MS);
    });
    expect(container.textContent).toContain("Image unavailable");
    await act(async () => resolveLate({ mimeType: "image/png", data: "aGk=" }));
    expect(container.textContent).toContain("Image unavailable");
    expect(create).not.toHaveBeenCalled();
    await unmount();
    await render([[ref(3)]]);
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(2);
    expect(imageSources()[0]?.startsWith("blob:")).toBe(true);
  });
});
