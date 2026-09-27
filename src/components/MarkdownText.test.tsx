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

    expect(markup).toContain('class="plan-markdown-heading plan-markdown-heading-3"');
    expect(markup).toContain("<code>C:\\Users\\x\\design-sandbox</code>");
    expect(markup).toContain("<strong>bold</strong>");
    expect(markup).toContain("<ul>");
    expect(markup).toContain("<li>first item</li>");
    expect(markup).toContain(
      "<pre><code>const path = `C:\\Users\\x\\design-sandbox`;</code></pre>",
    );
  });

  it("honors Markdown backslash escapes without consuming following delimiters", () => {
    const markup = renderToStaticMarkup(
      <MarkdownText text={String.raw`\*literal\* and \`tick\``} />,
    );

    expect(markup).toContain("*literal* and `tick`");
    expect(markup).not.toContain("<em>");
    expect(markup).not.toContain("<code>");
  });

  it("does not pair escaped ticks around a Windows path as inline code", () => {
    const markup = renderToStaticMarkup(
      <MarkdownText text={"literal \\`tick\\` then `C:\\Users\\x\\design-sandbox`"} />,
    );

    expect(markup).toContain("literal `tick` then ");
    expect(markup).toContain("<code>C:\\Users\\x\\design-sandbox</code>");
    expect(markup.match(/<code>/g)).toHaveLength(1);
  });
});
