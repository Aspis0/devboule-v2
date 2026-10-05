// The user bubble's thumbnail row: one pressable 48px thumbnail per image
// reference, in order, resolved through the stored-bytes read door. A
// reference that no longer resolves renders a quiet tile, never a break.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AttachmentReference } from "../../../lib/tauri";

const readMocks = vi.hoisted(() => ({ sessionAttachmentRead: vi.fn() }));

vi.mock("../../../lib/tauri", () => ({
  sessionAttachmentRead: readMocks.sessionAttachmentRead,
}));

import { resetChatImageCacheForTests } from "./chatImageCache";
import { ChatImageThumbnails } from "./ChatImageThumbnails";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

const REF_A: AttachmentReference = {
  sessionId: "s.owner.chat1",
  digest: "a".repeat(64),
  storedBytes: 12,
};
const REF_B: AttachmentReference = {
  sessionId: "s.owner.chat1",
  digest: "b".repeat(64),
  storedBytes: 34,
};

async function renderThumbnails(images: readonly AttachmentReference[]) {
  root = createRoot(container);
  await act(async () => {
    root.render(<ChatImageThumbnails images={images} />);
  });
  // The read resolves on mount; flush the microtask.
  await act(async () => {});
}

function thumbnailButtons(): HTMLButtonElement[] {
  return Array.from(container.querySelectorAll('button[aria-label^="Attached image"]'));
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  // Valid base64: the hook decodes the bytes for its Blob URL, so fixture
  // data must decode like the daemon's own replies do.
  readMocks.sessionAttachmentRead.mockImplementation(async () => ({
    mimeType: "image/png",
    data: "aGk=",
  }));
  vi.stubGlobal("IntersectionObserver", undefined);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  resetChatImageCacheForTests();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("ChatImageThumbnails", () => {
  it("renders one thumbnail button per reference, in order", async () => {
    await renderThumbnails([REF_A, REF_B]);
    const buttons = thumbnailButtons();
    expect(buttons).toHaveLength(2);
    expect(buttons[0]!.getAttribute("aria-label")).toBe("Attached image 1 of 2");
    expect(buttons[1]!.getAttribute("aria-label")).toBe("Attached image 2 of 2");
  });

  it("reads the stored bytes through the reference, never the original file", async () => {
    await renderThumbnails([REF_A, REF_B]);
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(2);
    expect(readMocks.sessionAttachmentRead).toHaveBeenNthCalledWith(1, REF_A);
    expect(readMocks.sessionAttachmentRead).toHaveBeenNthCalledWith(2, REF_B);
    const first = thumbnailButtons()[0]!.querySelector("img");
    expect(first?.getAttribute("src")?.startsWith("blob:")).toBe(true);
  });

  it("renders a quiet tile when a reference no longer resolves", async () => {
    readMocks.sessionAttachmentRead.mockRejectedValueOnce(new Error("gone"));
    await renderThumbnails([REF_A]);
    expect(thumbnailButtons()).toHaveLength(0);
    expect(container.textContent).toContain("Image unavailable");
  });

  it("opens the lightbox on click and closes it on Escape", async () => {
    await renderThumbnails([REF_A, REF_B]);
    await act(async () => {
      thumbnailButtons()[1]!.click();
    });
    expect(container.querySelector('[role="dialog"]')).not.toBeNull();
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector('[role="dialog"]')).toBeNull();
  });

  it("renders nothing for a text-only row", async () => {
    await renderThumbnails([]);
    expect(container.textContent).toBe("");
    expect(readMocks.sessionAttachmentRead).not.toHaveBeenCalled();
  });

  it("keeps a released URL for the next thumbnail instead of reading again", async () => {
    const revoke = vi.spyOn(URL, "revokeObjectURL");
    try {
      await renderThumbnails([REF_A]);
      const src = container.querySelector("img")?.getAttribute("src");
      expect(src?.startsWith("blob:")).toBe(true);
      await act(async () => {
        root.unmount();
      });
      expect(revoke).not.toHaveBeenCalled();
      await renderThumbnails([REF_A]);
      expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(1);
      expect(container.querySelector("img")?.getAttribute("src")).toBe(src);
    } finally {
      revoke.mockRestore();
    }
  });

  it("keeps a read that settles after unmount for the next thumbnail", async () => {
    let resolveRead!: (stored: { mimeType: string; data: string }) => void;
    readMocks.sessionAttachmentRead.mockImplementationOnce(
      () =>
        new Promise<{ mimeType: string; data: string }>((resolve) => {
          resolveRead = resolve;
        }),
    );
    await renderThumbnails([REF_A]);
    expect(container.querySelector("img")).toBeNull();
    await act(async () => {
      root.unmount();
    });
    await act(async () => {
      resolveRead({ mimeType: "image/png", data: "aGk=" });
    });
    await renderThumbnails([REF_A]);
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(1);
    expect(container.querySelector("img")?.getAttribute("src")?.startsWith("blob:")).toBe(true);
  });
});

describe("ChatImageThumbnails failure and focus", () => {
  it("shows the quiet tile when the image bytes fail to decode", async () => {
    await renderThumbnails([REF_A]);
    const img = container.querySelector("img");
    if (img === null) throw new Error("thumbnail image did not render");
    await act(async () => {
      img.dispatchEvent(new Event("error"));
    });
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("Image unavailable");
  });

  it("opens on thumbnail click with focus inside and returns focus to the thumbnail on Escape", async () => {
    await renderThumbnails([REF_A]);
    const thumb = thumbnailButtons()[0]!;
    await act(async () => {
      thumb.click();
    });
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog!.contains(document.activeElement)).toBe(true);
    await act(async () => {
      document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    expect(container.querySelector('[role="dialog"]')).toBeNull();
    expect(document.activeElement).toBe(thumb);
  });
});
