// The image lightbox: a modal dialog over the transcript's thumbnails.
// Escape closes it, arrows move through several images, focus stays inside
// while open and returns to the opener on close.
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
import { ImageLightbox } from "./ImageLightbox";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function refOf(digest: string): AttachmentReference {
  return { sessionId: "s.owner.chat1", digest, storedBytes: 12 };
}

const SOURCES = [
  { reference: refOf("a".repeat(64)), alt: "Attached image 1 of 2" },
  { reference: refOf("b".repeat(64)), alt: "Attached image 2 of 2" },
];

async function renderLightbox(props: {
  images?: typeof SOURCES;
  index?: number;
  openedIndex?: number;
  openedUrl?: string;
  opener?: HTMLElement | null;
  onIndexChange?: (index: number) => void;
  onClose?: () => void;
}) {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <div>
        <button type="button" data-testid="opener">
          opener
        </button>
        <ImageLightbox
          images={props.images ?? SOURCES}
          index={props.index ?? 0}
          openedIndex={props.openedIndex ?? 0}
          openedUrl={props.openedUrl ?? "blob:opened"}
          opener={props.opener}
          onIndexChange={props.onIndexChange ?? (() => undefined)}
          onClose={props.onClose ?? (() => undefined)}
        />
      </div>,
    );
  });
  await act(async () => {});
}

function press(key: string, shift = false) {
  document.dispatchEvent(
    new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, shiftKey: shift }),
  );
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.clearAllMocks();
  readMocks.sessionAttachmentRead.mockResolvedValue({ mimeType: "image/png", data: "aGk=" });
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  document.body.style.overflow = "";
  resetChatImageCacheForTests();
});

describe("ImageLightbox", () => {
  it("renders a modal dialog showing the opened image without reading again", async () => {
    await renderLightbox({ index: 1, openedIndex: 1, openedUrl: "blob:bbb" });
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog?.getAttribute("aria-modal")).toBe("true");
    const img = dialog?.querySelector("img");
    expect(img?.getAttribute("src")).toBe("blob:bbb");
    expect(img?.getAttribute("alt")).toBe("Attached image 2 of 2");
    expect(readMocks.sessionAttachmentRead).not.toHaveBeenCalled();
  });

  it("loads a sibling page on navigation", async () => {
    await renderLightbox({ index: 1, openedIndex: 0, openedUrl: "blob:aaa" });
    const img = container.querySelector('[role="dialog"] img');
    expect(img?.getAttribute("src")?.startsWith("blob:")).toBe(true);
    expect(readMocks.sessionAttachmentRead).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape", async () => {
    const onClose = vi.fn();
    await renderLightbox({ onClose });
    await act(async () => {
      press("Escape");
    });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("moves through several images with the arrow keys", async () => {
    const onIndexChange = vi.fn();
    await renderLightbox({ index: 0, onIndexChange });
    await act(async () => {
      press("ArrowRight");
    });
    expect(onIndexChange).toHaveBeenCalledWith(1);
  });

  it("ignores arrows for a single image", async () => {
    const onIndexChange = vi.fn();
    await renderLightbox({ images: [SOURCES[0]!], index: 0, onIndexChange });
    await act(async () => {
      press("ArrowRight");
      press("ArrowLeft");
    });
    expect(onIndexChange).not.toHaveBeenCalled();
  });

  it("keeps Tab focus inside the dialog", async () => {
    await renderLightbox({});
    const dialog = container.querySelector('[role="dialog"]') as HTMLElement;
    const buttons = Array.from(dialog.querySelectorAll("button"));
    expect(buttons.length).toBeGreaterThan(1);
    buttons[buttons.length - 1]!.focus();
    await act(async () => {
      press("Tab");
    });
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(document.activeElement).toBe(buttons[0]);
  });

  it("returns focus to the opener on close", async () => {
    const opener = document.createElement("button");
    opener.textContent = "thumbnail";
    document.body.appendChild(opener);
    opener.focus();
    await renderLightbox({});
    await act(async () => {
      root.unmount();
    });
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });

  it("shows a quiet tile when a navigated page does not resolve", async () => {
    readMocks.sessionAttachmentRead.mockRejectedValue(new Error("gone"));
    await renderLightbox({ index: 1, openedIndex: 0, openedUrl: "blob:aaa" });
    expect(container.querySelector('[role="dialog"]')?.textContent).toContain("Image unavailable");
  });
});

describe("ImageLightbox scroll and opener", () => {
  it("locks background scroll while open and restores it on close", async () => {
    document.body.style.overflow = "auto";
    await renderLightbox({});
    expect(document.body.style.overflow).toBe("hidden");
    await act(async () => {
      root.unmount();
    });
    expect(document.body.style.overflow).toBe("auto");
    document.body.style.overflow = "";
  });

  it("returns focus to the explicit opener when one is passed", async () => {
    const opener = document.createElement("button");
    opener.textContent = "thumbnail";
    document.body.appendChild(opener);
    root = createRoot(container);
    await act(async () => {
      root.render(
        <ImageLightbox
          images={SOURCES}
          index={0}
          openedIndex={0}
          openedUrl="blob:aaa"
          onIndexChange={() => undefined}
          onClose={() => undefined}
          opener={opener}
        />,
      );
    });
    await act(async () => {
      root.unmount();
    });
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
});

describe("two open viewers", () => {
  let bottom: HTMLDivElement;
  let top: HTMLDivElement;
  let bottomRoot: Root;
  let topRoot: Root;

  async function renderViewer(
    host: HTMLDivElement,
    props: { onClose: () => void; onIndexChange?: (index: number) => void },
  ): Promise<Root> {
    const viewerRoot = createRoot(host);
    await act(async () => {
      viewerRoot.render(
        <ImageLightbox
          images={SOURCES}
          index={0}
          openedIndex={0}
          openedUrl="blob:aaa"
          onIndexChange={props.onIndexChange ?? (() => undefined)}
          onClose={props.onClose}
        />,
      );
    });
    await act(async () => {});
    return viewerRoot;
  }

  beforeEach(() => {
    bottom = document.createElement("div");
    top = document.createElement("div");
    document.body.append(bottom, top);
  });

  afterEach(async () => {
    await act(async () => {
      bottomRoot?.unmount();
      topRoot?.unmount();
    });
    bottom.remove();
    top.remove();
  });

  it("Escape closes only the top viewer", async () => {
    const closeBottom = vi.fn();
    const closeTop = vi.fn();
    bottomRoot = await renderViewer(bottom, { onClose: closeBottom });
    topRoot = await renderViewer(top, { onClose: closeTop });
    await act(async () => {
      press("Escape");
    });
    expect(closeTop).toHaveBeenCalledTimes(1);
    expect(closeBottom).not.toHaveBeenCalled();
  });

  it("keeps the scroll lock until the last viewer closes", async () => {
    bottomRoot = await renderViewer(bottom, { onClose: () => undefined });
    topRoot = await renderViewer(top, { onClose: () => undefined });
    expect(document.body.style.overflow).toBe("hidden");
    await act(async () => {
      topRoot.unmount();
    });
    expect(document.body.style.overflow).toBe("hidden");
    await act(async () => {
      bottomRoot.unmount();
    });
    expect(document.body.style.overflow).toBe("");
  });
});

describe("ImageLightbox decode errors", () => {
  it("shows the quiet tile when the opened image fails to decode", async () => {
    await renderLightbox({ index: 0, openedIndex: 0, openedUrl: "blob:broken" });
    const img = container.querySelector('[role="dialog"] img');
    if (img === null) throw new Error("lightbox image did not render");
    await act(async () => {
      img.dispatchEvent(new Event("error"));
    });
    expect(container.querySelector('[role="dialog"] img')).toBeNull();
    expect(container.querySelector('[role="dialog"]')?.textContent).toContain("Image unavailable");
  });

  it("shows the quiet tile when a navigated image fails to decode", async () => {
    await renderLightbox({ index: 1, openedIndex: 0, openedUrl: "blob:aaa" });
    const img = container.querySelector('[role="dialog"] img');
    if (img === null) throw new Error("lightbox image did not render");
    await act(async () => {
      img.dispatchEvent(new Event("error"));
    });
    expect(container.querySelector('[role="dialog"] img')).toBeNull();
    expect(container.querySelector('[role="dialog"]')?.textContent).toContain("Image unavailable");
  });
});
