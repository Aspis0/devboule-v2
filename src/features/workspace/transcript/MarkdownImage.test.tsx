// A markdown image an agent wrote: the path must resolve inside the
// workspace, the bytes come from the workspace's own file read, and the
// picture is the same thumbnail + viewer every other chat picture uses.
// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatFileLinks } from "../../../lib/chatFilePaths";
import { MarkdownText } from "../../../components/MarkdownText";
import { MarkdownImage } from "./MarkdownImage";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ROOT = "C:\\work\\shop";

let container: HTMLDivElement;
let root: Root;

function linksOf(readImage?: ChatFileLinks["readImage"]): ChatFileLinks {
  return { root: ROOT, open: async () => "file", readImage };
}

async function render(text: string, links: ChatFileLinks, renderImage = true) {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <MarkdownText
        text={text}
        fileLinks={links}
        renderImage={
          renderImage
            ? (target, alt, key) => (
                <MarkdownImage key={key} target={target} alt={alt} links={links} />
              )
            : undefined
        }
      />,
    );
  });
  await act(async () => {});
}

function thumb(): HTMLButtonElement | null {
  return container.querySelector<HTMLButtonElement>(".workspace-chat-image-thumb");
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
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
