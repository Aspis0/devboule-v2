import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { parseMarkdownText } from "./markdownParser";

function markup(text: string): string {
  return renderToStaticMarkup(<div>{parseMarkdownText(text)}</div>);
}

describe("markdownParser tables", () => {
  it("renders a pipe table as a real table, never as pipes", () => {
    const out = markup("| Name | Value |\n|---|---|\n| port | 8080 |");

    expect(out).toContain("<table");
    expect(out).toContain("<th>Name</th>");
    expect(out).toContain("<td>port</td>");
    expect(out).toContain("<td>8080</td>");
  });

  it("aligns cells from the delimiter row's colons", () => {
    const out = markup("| a | b | c | d |\n|:---|---:|:---:|---|\n| 1 | 2 | 3 | 4 |");

    expect(out).toContain('style="text-align:left"');
    expect(out).toContain('style="text-align:right"');
    expect(out).toContain('style="text-align:center"');
    // The colon-less column carries no alignment style at all.
    expect(out).not.toContain('style="text-align:none"');
  });

  it("renders an escaped pipe as the cell's own character", () => {
    const out = markup("| a \\| b | c |\n|---|---|\n| x | y |");

    expect(out).toContain("<th>a | b</th>");
    expect(out).toContain("<th>c</th>");
  });

  it("pads a short row with empty cells and drops a long row's extras", () => {
    const out = markup("| a | b | c |\n|---|---|---|\n| one |\n| 1 | 2 | 3 | 4 |");
    const bodies = out.match(/<tr>/g) ?? [];

    expect(bodies).toHaveLength(3);
    expect((out.match(/<td>/g) ?? []).length).toBe(6);
  });

  it("falls back to paragraphs without a delimiter row", () => {
    const out = markup("| a | b |\n| c | d |");

    expect(out).not.toContain("<table");
    expect(out).toContain("| a | b |");
  });

  it("falls back to paragraphs when the delimiter row's cell count differs", () => {
    const out = markup("| a | b |\n|---|---|---|\n| 1 | 2 |");

    expect(out).not.toContain("<table");
    expect(out).toContain("| a | b |");
  });

  it("reads a pipe-less pair as text, not a setext heading or a table", () => {
    const out = markup("Title\n---");

    expect(out).not.toContain("<table");
    expect(out).toContain("Title ---");
  });

  it("keeps a table inside a code fence as code", () => {
    const out = markup("```\n| a | b |\n|---|---|\n```");

    expect(out).not.toContain("<table");
    expect(out).toContain('<div class="copyblock">');
    expect(out).toContain("| a | b |\n|---|---|");
  });

  it("lets a table interrupt a paragraph", () => {
    const out = markup("Leading text\n| a | b |\n|---|---|\n| 1 | 2 |");

    expect(out).toContain("<p>Leading text</p>");
    expect(out).toContain("<table");
  });

  it("wraps the table in a scroll wrapper that stays plain until it overflows", () => {
    const out = markup("| a | b |\n|---|---|\n| 1 | 2 |");

    expect(/<div[^>]*plan-markdown-table-scroll[^>]*><table/.exec(out)).not.toBeNull();
    // Static markup runs no effects, so the region role and tab stop are
    // the live wrapper's to grow (TableScrollRegion.test.tsx), never these.
    expect(out).not.toContain('tabindex="0"');
    expect(out).not.toContain('role="region"');
    expect(out).toContain("</tbody></table></div>");
  });

  it("never throws on malformed tables and pipe soup", () => {
    const inputs = [
      "|",
      "||",
      "|||",
      "| |",
      "|---|",
      "a|",
      "|a",
      "|\n|---|",
      "| a |",
      "| a |\n|",
      "| a |\n" + "-".repeat(500),
      "\\|",
      "|\\|",
      "| \\| | \\| |\n|---|---|",
      "![",
      "![](",
      "[a](b",
      "| `code | span |` |\n|---|",
      "| a |\n|:--:|\n| only header and delimiter",
    ];
    for (const input of inputs) {
      expect(() => markup(input)).not.toThrow();
    }
  });
});

describe("markdownParser images", () => {
  it("renders an image's alt text and never an img element", () => {
    const out = markup("see ![map](docs/img.png) here");

    expect(out).not.toContain("<img");
    expect(out).toContain('class="plan-markdown-image"');
    expect(out).toContain(">map</span>");
  });

  it("never loads a remote image either", () => {
    const out = markup("![pic](https://example.com/x.png)");

    expect(out).not.toContain("<img");
    expect(out).toContain(">pic</span>");
  });

  it("renders an empty alt as the muted word image, never the raw target", () => {
    const out = markup("see ![](docs/img.png) here");

    expect(out).not.toContain("<img");
    expect(out).not.toContain("![](");
    expect(out).not.toContain("docs/img.png");
    expect(out).toContain(">image</span>");
  });
});

describe("markdownParser safety", () => {
  it("keeps a javascript: link as literal text", () => {
    const out = markup("[click](javascript:alert(1))");

    expect(out).not.toContain("<a ");
    expect(out).toContain("[click](javascript:alert(1))");
  });

  it("keeps a relative link as literal text", () => {
    const out = markup("[docs](docs/index.md)");

    expect(out).not.toContain("<a ");
    expect(out).toContain("[docs](docs/index.md)");
  });

  it("renders raw HTML as text, never as elements", () => {
    const out = markup("<script>alert(1)</script>\n\n<b>bold</b>");

    expect(out).not.toContain("<script");
    expect(out).not.toContain("<b>");
    expect(out).toContain("&lt;script&gt;alert(1)&lt;/script&gt;");
  });
});

describe("markdownParser link targets", () => {
  // The scheme battery: a target the allowlist refuses must survive the
  // balanced-paren scan whole and render as the literal text it came as.
  it.each([
    "javascript:alert(1)",
    "JAVASCRIPT:alert(1)",
    "JaVaScRiPt:alert(1)",
    " javascript:alert(1)",
    "\u0001javascript:alert(1)",
    "data:text/html,<script>alert(1)</script>",
    "vbscript:MsgBox(1)",
    "file:///etc/passwd",
    "//host/spoof",
  ])("keeps [%j] literal, never an anchor", (target) => {
    const out = markup(`[click](${target})`);

    expect(out).not.toContain("<a ");
    expect(out).toContain("[click](");
  });

  it("keeps an entity-colon scheme literal too", () => {
    const out = markup("[click](javascript&#58;alert(1))");

    expect(out).not.toContain("<a ");
  });

  it("keeps a parenthesised https target whole", () => {
    const out = markup("[t](https://en.wikipedia.org/wiki/A_(B))");

    expect(out).toContain('href="https://en.wikipedia.org/wiki/A_(B)"');
  });

  // CommonMark: a bare destination ends at the first whitespace, so a `)`
  // past one closes nothing — the link is refused outright, never an href
  // that swallows the prose.
  it("refuses a link whose destination would run past a space", () => {
    const out = markup("[a](https://e.com/x(y) some prose ) tail text");

    expect(out).not.toContain("<a ");
    expect(out).not.toContain("href=");
    expect(out).toContain("[a](https://e.com/x(y) some prose ) tail text");
  });

  it.each([
    ["[a](http://e.com/a<b)", "[a](http://e.com/a&lt;b)"],
    ["[a](http://e.com/a b)", "[a](http://e.com/a b)"],
    ["[a](http://e.com/a\tb)", "[a](http://e.com/a\tb)"],
    ["[a](http://e.com/a\rb)", "[a](http://e.com/a\rb)"],
    ["[a](http://e.com/a\fb)", "[a](http://e.com/a\fb)"],
    ["[a](http://e.com/a\nb)", "[a](http://e.com/a b)"],
    ["[a](http://e.com/a\\ b)", "[a](http://e.com/a\\ b)"],
    ['[a](http://e.com "Title")', "[a](http://e.com &quot;Title&quot;)"],
    ["![a](https://e.com/x(y) prose )", "![a](https://e.com/x(y) prose )"],
  ])("refuses %j as literal text", (input, literal) => {
    const out = markup(input);

    expect(out).not.toContain("<a ");
    expect(out).not.toContain("href=");
    expect(out).toContain(literal);
  });

  it("still links http, https and mailto after the scan", () => {
    expect(markup("[a](http://e.com/x_(y))")).toContain('<a href="http://e.com/x_(y)"');
    expect(markup("[a](HTTPS://e.com)")).toContain('href="HTTPS://e.com"');
    expect(markup("[a](mailto:a@b.c)")).toContain('href="mailto:a@b.c"');
  });

  it("consumes a parenthesised image target with no leak and no fetch", () => {
    const out = markup("![map](https://e.com/img_(1).png)");

    expect(out).not.toContain("<img");
    expect(out).toContain(">map</span>");
    expect(out).not.toContain(".png)");
  });

  it("keeps an empty target literal", () => {
    const out = markup("[a]()");

    expect(out).not.toContain("<a ");
    expect(out).toContain("[a]()");
  });
});
