import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { inline } from "./markdownInline";

const headOutput = [
  {
    text: "",
    expected: "<div></div>",
  },
  {
    text: "plain words",
    expected: "<div>plain words</div>",
  },
  {
    text: "src/a.ts",
    expected: "<div>src/a.ts</div>",
  },
  {
    text: "`src/a.ts`",
    expected: "<div><code>src/a.ts</code></div>",
  },
  {
    text: "`code\nspan`",
    expected: "<div><code>code\nspan</code></div>",
  },
  {
    text: "**bo\nld**",
    expected: "<div><strong>bo\nld</strong></div>",
  },
  {
    text: "*em\nphasis*",
    expected: "<div><em>em\nphasis</em></div>",
  },
  {
    text: "a\nb",
    expected: "<div>a\nb</div>",
  },
  {
    text: "\n\n\n",
    expected: "<div>\n\n\n</div>",
  },
  {
    text: "before\n`code`\nafter",
    expected: "<div>before\n<code>code</code>\nafter</div>",
  },
  {
    text: "**use `code\nspan` now**",
    expected: "<div><strong>use <code>code\nspan</code> now</strong></div>",
  },
  {
    text: "*use `code\nspan` now*",
    expected: "<div><em>use <code>code\nspan</code> now</em></div>",
  },
  {
    text: "\\*literal\\*\nnext",
    expected: "<div>*literal*\nnext</div>",
  },
  {
    text: "[docs](https://e.com)",
    expected: '<div><a href="https://e.com" target="_blank" rel="noreferrer">docs</a></div>',
  },
  {
    text: "[la\nbel](https://e.com)",
    expected: '<div><a href="https://e.com" target="_blank" rel="noreferrer">la\nbel</a></div>',
  },
  {
    text: "[a](src/a.ts)",
    expected: "<div>[a](src/a.ts)</div>",
  },
  {
    text: "[a](javascript:alert(1))",
    expected: "<div>[a](javascript:alert(1))</div>",
  },
  {
    text: "![alt\ntext](https://e.com/image.png)",
    expected: '<div><span class="plan-markdown-image">alt\ntext</span></div>',
  },
  {
    text: "![](https://e.com/image.png)",
    expected: '<div><span class="plan-markdown-image">image</span></div>',
  },
  {
    text: "`C:\\repo\\src\\a.ts`",
    expected: "<div><code>C:\\repo\\src\\a.ts</code></div>",
  },
  {
    text: "`unclosed\ncode",
    expected: "<div>`unclosed\ncode</div>",
  },
  {
    text: "**unclosed\nbold",
    expected: "<div>**unclosed\nbold</div>",
  },
  {
    text: "*unclosed\nem",
    expected: "<div>*unclosed\nem</div>",
  },
  {
    text: "[unclosed\nlabel",
    expected: "<div>[unclosed\nlabel</div>",
  },
  {
    text: "before\r\nafter",
    expected: "<div>before\r\nafter</div>",
  },
  {
    text: "`code\r\nspan`",
    expected: "<div><code>code\r\nspan</code></div>",
  },
  {
    text: '<tag> & "quote"',
    expected: "<div>&lt;tag&gt; &amp; &quot;quote&quot;</div>",
  },
  {
    text: "**bold** and *em* and `code`",
    expected: "<div><strong>bold</strong> and <em>em</em> and <code>code</code></div>",
  },
  {
    text: "[docs](https://e.com/wiki/A_(B))",
    expected:
      '<div><a href="https://e.com/wiki/A_(B)" target="_blank" rel="noreferrer">docs</a></div>',
  },
  {
    text: "`src/a\\.ts`\nnext",
    expected: "<div><code>src/a\\.ts</code>\nnext</div>",
  },
];

describe("feature-off inline compatibility", () => {
  it.each(headOutput)("preserves markup for $text", ({ text, expected }) => {
    expect(renderToStaticMarkup(<div>{inline(text)}</div>)).toBe(expected);
    expect(renderToStaticMarkup(<div>{inline(text, null)}</div>)).toBe(expected);
  });
});
