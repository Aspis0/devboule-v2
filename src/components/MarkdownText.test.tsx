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
});
