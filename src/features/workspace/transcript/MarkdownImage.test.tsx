// A markdown image an agent wrote: the path must resolve inside the
// workspace, the bytes come from the workspace's own file read, and the
// picture is the same thumbnail + viewer every other chat picture uses.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatFileLinks } from "../../../lib/chatFilePaths";
import { MarkdownText } from "../../../components/MarkdownText";
import { MarkdownImage, sharedImageReader } from "./MarkdownImage";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ROOT = "C:\\work\\shop";

let container: HTMLDivElement;
let root: Root;

function linksOf(readImage?: ChatFileLinks["readImage"]): ChatFileLinks {
  return { root: ROOT, open: async () => "file", readImage };
}

async function render(text: string, links: ChatFileLinks, withReader = true) {
  const read = links.readImage === undefined ? undefined : sharedImageReader(links.readImage);
  root = createRoot(container);
  await act(async () => {
    root.render(
      <MarkdownText
        text={text}
        fileLinks={links}
        renderImage={
          withReader && read !== undefined
            ? (target, alt, key) => (
                <MarkdownImage key={key} target={target} alt={alt} links={links} read={read} />
              )
            : undefined
        }
      />,
    );
  });
  await act(async () => {});
}

/** Holds each observer's targets until a test says they came near the viewport. */
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
  static intersectAll() {
    for (const observer of FakeIntersectionObserver.instances) {
      const entries = [...observer.targets].map(
        (target) => ({ target, isIntersecting: true }) as IntersectionObserverEntry,
      );
      if (entries.length > 0)
        observer.callback(entries, observer as unknown as IntersectionObserver);
    }
  }
}

function thumb(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>(".workspace-chat-image-thumb");
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  vi.stubGlobal("IntersectionObserver", undefined);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  FakeIntersectionObserver.instances = [];
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("MarkdownImage", () => {
  it("reads a workspace-relative path and draws it as a thumbnail", async () => {
    const readImage = vi.fn(async () => ({ mimeType: "image/png", data: "aGk=" }));
    await render("Here it is\n\n![](mockups/shot.png)", linksOf(readImage));
    expect(readImage).toHaveBeenCalledWith("mockups/shot.png");
    const button = thumb();
    expect(button).not.toBeNull();
    expect(button!.getAttribute("aria-label")).toBe("Image");
    expect(button!.querySelector("img")?.getAttribute("src")).toMatch(/^blob:/);
  });

  it("resolves an absolute path inside the root to its workspace-relative spelling", async () => {
    const readImage = vi.fn(async () => ({ mimeType: "image/jpeg", data: "aGk=" }));
    await render(`![the shot](${ROOT}\\mockups\\shot.jpg)`, linksOf(readImage));
    expect(readImage).toHaveBeenCalledWith("mockups/shot.jpg");
    expect(thumb()!.getAttribute("aria-label")).toBe("the shot");
  });

  it("leaves a path outside the workspace as the alt text and never reads it", async () => {
    const readImage = vi.fn(async () => ({ mimeType: "image/png", data: "aGk=" }));
    await render("![](C:\\elsewhere\\shot.png)", linksOf(readImage));
    expect(readImage).not.toHaveBeenCalled();
    expect(thumb()).toBeNull();
    expect(container.querySelector(".plan-markdown-image")?.textContent).toBe("image");
  });

  it("leaves a remote destination as the alt text", async () => {
    const readImage = vi.fn(async () => ({ mimeType: "image/png", data: "aGk=" }));
    await render("![](https://example.invalid/shot.png)", linksOf(readImage));
    expect(readImage).not.toHaveBeenCalled();
    expect(thumb()).toBeNull();
  });

  it("shows the unavailable tile when the read refuses", async () => {
    await render(
      "![](mockups/shot.png)",
      linksOf(async () => null),
    );
    expect(thumb()).toBeNull();
    expect(container.querySelector(".workspace-chat-image-unavailable")?.textContent).toBe(
      "Image unavailable",
    );
  });

  it("opens the viewer over the picture on a press", async () => {
    await render(
      "![](mockups/shot.png)",
      linksOf(async () => ({ mimeType: "image/png", data: "aGk=" })),
    );
    const button = thumb()!;
    await act(async () => {
      button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const dialog = container.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog!.querySelector(".chat-image-lightbox-image")?.getAttribute("src")).toMatch(
      /^blob:/,
    );
  });

  it("keeps the alt text where the surface has no image reader", async () => {
    await render("![](mockups/shot.png)", linksOf(undefined));
    expect(thumb()).toBeNull();
    expect(container.querySelector(".plan-markdown-image")?.textContent).toBe("image");
  });
});

describe("MarkdownImage reads", () => {
  const PICTURE = { mimeType: "image/png", data: "aGk=" };

  it("waits for the picture to come near the viewport before reading it", async () => {
    vi.stubGlobal("IntersectionObserver", FakeIntersectionObserver);
    const readImage = vi.fn(async () => PICTURE);
    await render("![](mockups/shot.png)", linksOf(readImage));
    expect(readImage).not.toHaveBeenCalled();
    await act(async () => FakeIntersectionObserver.intersectAll());
    await act(async () => {});
    expect(readImage).toHaveBeenCalledTimes(1);
    expect(thumb()).not.toBeNull();
  });

  it("reads a picture a text repeats once", async () => {
    const readImage = vi.fn(async () => PICTURE);
    await render(
      "![](mockups/shot.png) and ![](mockups/shot.png) and ![](mockups/shot.png)",
      linksOf(readImage),
    );
    expect(readImage).toHaveBeenCalledTimes(1);
    expect(container.querySelectorAll(".workspace-chat-image-thumb")).toHaveLength(3);
  });

  it("draws at most eight pictures a text and leaves the rest as alt text", async () => {
    const readImage = vi.fn(async () => PICTURE);
    const text = Array.from(
      { length: 10 },
      (_, index) => `![p${index}](mockups/shot-${index}.png)`,
    ).join(" ");
    await render(text, linksOf(readImage));
    expect(readImage).toHaveBeenCalledTimes(8);
    expect(container.querySelectorAll(".workspace-chat-image-thumb")).toHaveLength(8);
    expect(container.querySelectorAll(".plan-markdown-image")).toHaveLength(2);
  });
});
