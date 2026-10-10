// The composer's image picker: picked image files ride the send as prompt
// attachments, preview until sent, and remove cleanly. Text-only sends keep
// the arity every existing caller expects.
// @vitest-environment happy-dom

import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PromptAttachment } from "../../types/ipc";
import { WorkspaceComposer } from "./WorkspaceComposer";
import { composerProps } from "./composerTestKit";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;

function pngFile(name = "photo.png"): File {
  return new File([new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10])], name, {
    type: "image/png",
  });
}

function gifFile(name = "loop.gif"): File {
  return new File([new TextEncoder().encode("GIF89a")], name, { type: "image/gif" });
}

function webpFile(name = "photo.webp"): File {
  return new File([new TextEncoder().encode("RIFF....WEBP")], name, { type: "image/webp" });
}

async function renderComposer(
  onSend: (text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>,
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
) {
  const mocks = { onSend: vi.fn(), onQueue: vi.fn() };
  root = createRoot(container);
  await act(async () => {
    root.render(<WorkspaceComposer {...composerProps(mocks, overrides)} onSend={onSend} />);
  });
}

function imageInput(): HTMLInputElement {
  const input = container.querySelector<HTMLInputElement>(
    'input[data-testid="composer-image-input"]',
  );
  if (input === null) throw new Error("composer image input did not render");
  return input;
}

function pickFiles(...files: File[]) {
  const input = imageInput();
  Object.defineProperty(input, "files", { value: files, configurable: true });
  input.dispatchEvent(new Event("change", { bubbles: true }));
}

async function typeText(text: string) {
  const textarea = container.querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="Message the agent"]',
  );
  if (textarea === null) throw new Error("composer textarea did not render");
  const setValue = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value")!.set!;
  await act(async () => {
    setValue.call(textarea, text);
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
});

describe("WorkspaceComposer images", () => {
  it("sends picked images as attachments beside the text", async () => {
    const onSend = vi.fn();
    await renderComposer(onSend);
    pickFiles(pngFile());
    await act(async () => {});
    expect(container.querySelector('[data-testid="composer-image-preview"]')).not.toBeNull();
    await typeText("look at this");
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    expect(onSend).toHaveBeenCalledTimes(1);
    const [text, attachments] = onSend.mock.calls[0] as [string, readonly PromptAttachment[]];
    expect(text).toBe("look at this");
    expect(attachments).toHaveLength(1);
    expect(attachments[0]!.mimeType).toBe("image/png");
    expect(attachments[0]!.name).toBe("photo.png");
    expect(typeof attachments[0]!.data).toBe("string");
  });

  it("removes a picked image before sending", async () => {
    const onSend = vi.fn();
    await renderComposer(onSend);
    pickFiles(pngFile());
    await act(async () => {});
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[aria-label^="Remove attached image"]')!
        .click();
    });
    expect(container.querySelector('[data-testid="composer-image-preview"]')).toBeNull();
    await typeText("no images after all");
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    const [, attachments] = onSend.mock.calls[0] as [string, readonly PromptAttachment[]];
    expect(attachments).toHaveLength(0);
  });

  it("refuses a non-image file without adding a preview", async () => {
    const onSend = vi.fn();
    await renderComposer(onSend);
    const text = new File(["hello"], "notes.txt", { type: "text/plain" });
    pickFiles(text);
    await act(async () => {});
    expect(container.querySelector('[data-testid="composer-image-preview"]')).toBeNull();
  });

  it("routes GIF and WebP to the file route for a daemon that did not agree them", async () => {
    const onAddFiles = vi.fn();
    await renderComposer(vi.fn(), { onAddFiles });
    expect(imageInput().accept).toBe("");
    pickFiles(gifFile(), webpFile());
    await act(async () => {});
    expect(container.querySelector('[data-testid="composer-image-preview"]')).toBeNull();
    expect(onAddFiles).toHaveBeenCalledTimes(1);
    const picked = onAddFiles.mock.calls[0]![0] as readonly File[];
    expect(picked.map((file) => file.name)).toEqual(["loop.gif", "photo.webp"]);
  });

  it("keeps the accepted image and routes the rest to the file route", async () => {
    const onSend = vi.fn();
    const onAddFiles = vi.fn();
    await renderComposer(onSend, { onAddFiles });
    pickFiles(pngFile(), gifFile());
    await act(async () => {});
    expect(container.querySelectorAll('[data-testid="composer-image-preview"]')).toHaveLength(1);
    const routed = onAddFiles.mock.calls[0]![0] as readonly File[];
    expect(routed.map((file) => file.name)).toEqual(["loop.gif"]);

    // An image over the image route's ceiling is a file too, never a
    // refusal: the file route can carry it and the agent can read it.
    const oversized = new File([new Uint8Array(128 * 1024 + 1)], "big.png", { type: "image/png" });
    pickFiles(pngFile("second.png"), oversized);
    await act(async () => {});
    expect(container.querySelectorAll('[data-testid="composer-image-preview"]')).toHaveLength(2);
    const second = onAddFiles.mock.calls[1]![0] as readonly File[];
    expect(second.map((file) => file.name)).toEqual(["big.png"]);
  });

  it("sends a picked GIF and WebP once the daemon agreed them", async () => {
    const onSend = vi.fn();
    const onAddFiles = vi.fn();
    await renderComposer(onSend, { gifWebpSupported: true, onAddFiles });
    expect(imageInput().accept).toBe("");
    pickFiles(gifFile(), webpFile());
    await act(async () => {});
    expect(onAddFiles).not.toHaveBeenCalled();
    expect(container.querySelectorAll('[data-testid="composer-image-preview"]')).toHaveLength(2);
    await typeText("an animation and a photo");
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    const [, attachments] = onSend.mock.calls[0] as [string, readonly PromptAttachment[]];
    expect(attachments.map((attachment) => attachment.mimeType)).toEqual([
      "image/gif",
      "image/webp",
    ]);
  });
});

describe("WorkspaceComposer handed-back images", () => {
  it("restores handed-back images as previews beside the handed-back text", async () => {
    const mocks = { onSend: vi.fn(), onQueue: vi.fn() };
    root = createRoot(container);
    const restored: PromptAttachment = { name: "photo.png", mimeType: "image/png", data: "aGk=" };
    await act(async () => {
      root.render(
        <WorkspaceComposer
          {...composerProps(mocks)}
          onSend={mocks.onSend}
          restoreDraft={{ text: "look at this", images: [restored], focus: false, nonce: 7 }}
        />,
      );
    });
    expect(container.querySelector('[data-testid="composer-image-preview"]')).not.toBeNull();
    const textarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    expect(textarea?.value).toBe("look at this");
  });
});

describe("WorkspaceComposer image sends", () => {
  async function renderSending(
    onSend: (text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>,
    overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
  ) {
    const mocks = { onSend: vi.fn(), onQueue: vi.fn() };
    root = createRoot(container);
    await act(async () => {
      root.render(<WorkspaceComposer {...composerProps(mocks)} onSend={onSend} {...overrides} />);
    });
  }

  function imageInput(): HTMLInputElement {
    const input = container.querySelector<HTMLInputElement>(
      'input[data-testid="composer-image-input"]',
    );
    if (input === null) throw new Error("composer image input did not render");
    return input;
  }

  async function pickAndType() {
    Object.defineProperty(imageInput(), "files", {
      value: [pngFile()],
      configurable: true,
    });
    await act(async () => {
      imageInput().dispatchEvent(new Event("change", { bubbles: true }));
    });
    await act(async () => {});
    await typeText("look at this");
  }

  it("disables the picker while a send is in flight", async () => {
    let resolveSend!: (sent: boolean) => void;
    await renderSending(
      () =>
        new Promise<boolean>((resolve) => {
          resolveSend = resolve;
        }),
    );
    await pickAndType();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    expect(imageInput().disabled).toBe(true);
    expect(
      container.querySelector<HTMLButtonElement>('button[aria-label="Attach file"]')!.disabled,
    ).toBe(true);
    expect(container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.disabled).toBe(
      true,
    );
    await act(async () => {
      resolveSend(true);
    });
    expect(imageInput().disabled).toBe(false);
  });

  it("clears the picked images on success", async () => {
    await renderSending(async () => true);
    await pickAndType();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    await act(async () => {});
    expect(container.querySelector('[data-testid="composer-image-preview"]')).toBeNull();
  });

  it("keeps the picked images and re-enables the picker on failure", async () => {
    const onSend = vi.fn(async () => false);
    await renderSending(onSend);
    await pickAndType();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    await act(async () => {});
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(container.querySelector('[data-testid="composer-image-preview"]')).not.toBeNull();
    expect(imageInput().disabled).toBe(false);
  });

  it("keeps the text with the images on failure", async () => {
    const onSend = vi.fn(async () => false);
    await renderSending(onSend, { restoreTextOnFailure: true });
    await pickAndType();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    await act(async () => {});
    // The submission stays whole: images kept above, and the words handed
    // back instead of dropped with them.
    expect(container.querySelector('[data-testid="composer-image-preview"]')).not.toBeNull();
    const textarea = container.querySelector<HTMLTextAreaElement>(
      'textarea[aria-label="Message the agent"]',
    );
    expect(textarea?.value).toBe("look at this");
  });

  it("ignores a second submit while a send is in flight", async () => {
    const onSend = vi.fn(
      () =>
        new Promise<boolean>(() => {
          // Never settles: the second submit must not reach the sender.
        }),
    );
    await renderSending(onSend);
    await pickAndType();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    await typeText("more words");
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send"]')!.click();
    });
    expect(onSend).toHaveBeenCalledTimes(1);
  });
});

describe("WorkspaceComposer image cap", () => {
  it("caps two overlapping picks at four images with a notice", async () => {
    const onSend = vi.fn(async () => true);
    const mocks = { onSend: vi.fn(), onQueue: vi.fn() };
    root = createRoot(container);
    await act(async () => {
      root.render(<WorkspaceComposer {...composerProps(mocks)} onSend={onSend} />);
    });
    const gates: Array<(buffer: ArrayBuffer) => void> = [];
    const deferredPng = (name: string): File => {
      const file = new File([new Uint8Array([137, 80, 78, 71])], name, { type: "image/png" });
      const gate = new Promise<ArrayBuffer>((resolve) => {
        gates.push(resolve);
      });
      Object.defineProperty(file, "arrayBuffer", { value: () => gate });
      return file;
    };
    const pick = async (...names: string[]) => {
      const input = container.querySelector<HTMLInputElement>(
        'input[data-testid="composer-image-input"]',
      );
      if (input === null) throw new Error("composer image input did not render");
      Object.defineProperty(input, "files", {
        value: names.map(deferredPng),
        configurable: true,
      });
      await act(async () => {
        input.dispatchEvent(new Event("change", { bubbles: true }));
      });
    };
    await pick("a.png", "b.png", "c.png");
    await pick("d.png", "e.png", "f.png");
    await act(async () => {
      for (const resolve of gates.splice(0)) resolve(new Uint8Array([137, 80, 78, 71]).buffer);
    });
    await act(async () => {});
    expect(container.querySelectorAll('[data-testid="composer-image-preview"]')).toHaveLength(4);
    expect(container.textContent).toContain("omit");
  });
});
