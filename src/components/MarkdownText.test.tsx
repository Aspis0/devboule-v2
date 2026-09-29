import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { MarkdownText } from "./MarkdownText";

describe("MarkdownText", () => {
  it("keeps unsafe links literal and gives plan headings an accessible role", () => {
    const markup = renderToStaticMarkup(
      <MarkdownText text={"# Plan\n\n[open](javascript:alert(1))"} />,
    );

    expect(markup).toContain('class="plan-markdown-heading plan-markdown-heading-1"');
    expect(markup).toContain('role="heading"');
    expect(markup).toContain('aria-level="1"');
    expect(markup).toContain("[open](javascript:alert(1))");
    expect(markup).not.toContain("<h1");
    expect(markup).not.toContain('href="javascript:');
  });

  it("handles 100 KB of unmatched brackets within a generous time bound", () => {
    const input = "[".repeat(100_000);
    const started = performance.now();
    const markup = renderToStaticMarkup(<MarkdownText text={input} />);

    expect(markup).toContain(input);
    expect(performance.now() - started).toBeLessThan(1_000);
  });

  // 128 KiB is the wire's file-read window, and a chat message carries it
  // uncapped: parsing one line must stay linear in the line's length, so
  // each candidate-heavy shape clears the same generous bar as above.
  it.each([
    { shape: "[a](", chunk: "[a](" },
    { shape: "[a](x", chunk: "[a](x" },
    { shape: "![a](", chunk: "![a](" },
    { shape: "the three mixed with spaces", chunk: "[a]( [a](x ![a](" },
  ])("renders 128 KiB of $shape in one line within a generous time bound", ({ chunk }) => {
    const input = chunk.repeat(Math.floor((128 * 1024) / chunk.length));
    const started = performance.now();
    const markup = renderToStaticMarkup(<MarkdownText text={input} />);

    expect(markup).toContain(input);
    expect(performance.now() - started).toBeLessThan(1_000);
  });

  it("renders the timeline Markdown elements and preserves Windows paths in code", () => {
    const markup = renderToStaticMarkup(
      <MarkdownText
        text={[
          "### Details",
          "A `C:\\Users\\x\\design-sandbox` path and **bold** text.",
          "- first item",
          "- second item",
          "```ts",
          "const path = `C:\\Users\\x\\design-sandbox`;",
          "```",
        ].join("\n")}
      />,
    );

    expect(markup).toBe(
      '<div><div role="heading" aria-level="3" class="plan-markdown-heading plan-markdown-heading-3">Details</div><p>A <code>C:\\Users\\x\\design-sandbox</code> path and <strong>bold</strong> text.</p><ul><li>first item</li><li>second item</li></ul><div class="codeblock-sample"><button type="button" class="copy-btn" aria-label="Copy code">Copy</button><span aria-live="polite" class="sr-only"></span><pre><code>const path = `C:\\Users\\x\\design-sandbox`;</code></pre></div></div>',
    );
  });

  it("honors Markdown backslash escapes without consuming following delimiters", () => {
    const markup = renderToStaticMarkup(
      <MarkdownText text={String.raw`\*literal\* and \_under\_ and \`tick\``} />,
    );

    expect(markup).toBe("<div><p>*literal* and _under_ and `tick`</p></div>");
  });

  it("renders an inline code paragraph without the closing delimiter", () => {
    const markup = renderToStaticMarkup(<MarkdownText text={"a `code` b"} />);

    expect(markup).toBe("<div><p>a <code>code</code> b</p></div>");
  });

  it("renders the acceptance Windows path without a stray closing tick", () => {
    const markup = renderToStaticMarkup(
      <MarkdownText text={"A `C:\\Users\\x\\design-sandbox` path"} />,
    );

    expect(markup).toBe("<div><p>A <code>C:\\Users\\x\\design-sandbox</code> path</p></div>");
  });

  it("keeps a trailing Windows path backslash inside the code span", () => {
    const markup = renderToStaticMarkup(
      <MarkdownText text={"Save under `C:\\temp\\` and `other` ok"} />,
    );

    expect(markup).toBe(
      "<div><p>Save under <code>C:\\temp\\</code> and <code>other</code> ok</p></div>",
    );
  });
});
