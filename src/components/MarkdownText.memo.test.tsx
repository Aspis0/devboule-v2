// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatFileLinks } from "../lib/chatFilePaths";

const parser = vi.hoisted(() => ({
  parseMarkdownText: vi.fn((text: string, _fileLinks?: ChatFileLinks | null) => text),
}));

vi.mock("./markdownParser", () => parser);

import { MarkdownText } from "./MarkdownText";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

function Messages({ second }: { second: string }) {
  return (
    <>
      <MarkdownText text="unchanged" />
      <MarkdownText text={second} />
    </>
  );
}

describe("memoized assistant Markdown", () => {
  let container: HTMLDivElement;
  let root: Root;

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    parser.parseMarkdownText.mockClear();
  });

  it("re-parses only the message whose streamed text changed", async () => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => root.render(<Messages second="first chunk" />));
    expect(parser.parseMarkdownText).toHaveBeenCalledTimes(2);

    await act(async () => root.render(<Messages second="second chunk" />));

    expect(parser.parseMarkdownText).toHaveBeenCalledTimes(3);
    expect(parser.parseMarkdownText.mock.calls.map(([text]) => text)).toEqual([
      "unchanged",
      "first chunk",
      "second chunk",
    ]);
  });

  it("does not re-parse unchanged text with stable fileLinks on re-render", async () => {
    const fileLinks: ChatFileLinks = { root: "/home/u/repo", open: vi.fn() };
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => root.render(<MarkdownText text="src/a.ts" fileLinks={fileLinks} />));
    expect(parser.parseMarkdownText).toHaveBeenCalledTimes(1);
    expect(parser.parseMarkdownText).toHaveBeenCalledWith("src/a.ts", fileLinks);

    await act(async () => root.render(<MarkdownText text="src/a.ts" fileLinks={fileLinks} />));
    expect(parser.parseMarkdownText).toHaveBeenCalledTimes(1);

    await act(async () => root.render(<MarkdownText text="src/b.ts" fileLinks={fileLinks} />));
    expect(parser.parseMarkdownText).toHaveBeenCalledTimes(2);
    expect(parser.parseMarkdownText).toHaveBeenLastCalledWith("src/b.ts", fileLinks);
  });
});
