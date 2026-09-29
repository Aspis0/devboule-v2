import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { parseMarkdownText } from "./markdownParser";

function markup(text: string): string {
  return renderToStaticMarkup(<div>{parseMarkdownText(text)}</div>);
}

/** 128 KiB is the wire's file-read window. Each chunk carries a strong
 * that really forms, plus unclosed `**`/`*`/backtick and an escaped star. */
function adversarial(bytes: number): string {
  const chunk = "**q `r` s** ***t* **\\* `\n\n";
  return chunk.repeat(Math.floor(bytes / chunk.length));
}

describe("inline constructs nested in emphasis", () => {
  it("renders a code span nested in bold", () => {
    const out = markup("**use `x` here**");

    expect(out).toContain("<strong>use <code>x</code> here</strong>");
  });

  it("renders a link nested in bold", () => {
    const out = markup("**see [docs](https://e.com) now**");

    expect(out).toContain(
      '<strong>see <a href="https://e.com" target="_blank" rel="noreferrer">docs</a> now</strong>',
    );
  });

  it("renders code and a link nested in emphasis", () => {
    const out = markup("*use `x` or [docs](https://e.com)*");

    expect(out).toContain(
      '<em>use <code>x</code> or <a href="https://e.com" target="_blank" rel="noreferrer">docs</a></em>',
    );
  });

  it("consumes a backslash escape inside bold the way plain text consumes it", () => {
    const out = markup("**esc \\* here**");

    expect(out).toContain("<strong>esc * here</strong>");
  });

  // Emphasis can never sit inside bold: a strong needs its closing ** right
  // after the first star of its content, so a star in the content cancels
  // the strong instead of nesting one.
  it("keeps emphasis out of bold", () => {
    const out = markup("**a *b* c**");

    expect(out).not.toContain("<strong>");
  });
});

describe("inline scanning work bound", () => {
  it("parses 128 KiB of nested and unclosed emphasis markers within a generous time bound", () => {
    const input = adversarial(128 * 1024);
    const started = performance.now();
    const out = markup(input);

    expect(out).toContain("<strong>q <code>r</code> s</strong>");
    // Loose on purpose: a loaded suite takes ~1.4 s here, a quadratic scan tens of seconds.
    expect(performance.now() - started).toBeLessThan(5_000);
  });
});
