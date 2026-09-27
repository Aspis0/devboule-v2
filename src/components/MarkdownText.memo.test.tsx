// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const parser = vi.hoisted(() => ({
  parseMarkdownText: vi.fn((text: string) => text),
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
});
