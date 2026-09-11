// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { ARTIFACT_EXPORT_FALLBACK_TITLE, buildStandaloneArtifactHtml } from "./artifactExport";

const APP_CSP_META =
  `<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; ` +
  `style-src 'unsafe-inline'; script-src 'none'; font-src 'none'" />`;

function countOccurrences(haystack: string, needle: string): number {
  return haystack.split(needle).length - 1;
}

describe("artifact export", () => {
  it("wraps a body fragment in a standalone shell with the message title", () => {
    const doc = buildStandaloneArtifactHtml(
      '<main class="card">Hello</main>',
      "Edited Index header",
    );

    expect(doc).toContain("<!DOCTYPE html>");
    expect(doc).toContain('<html lang="en">');
    expect(doc).toContain("<head>");
    expect(doc).toContain('<meta charset="utf-8">');
    expect(doc).toContain('<meta name="viewport"');
    expect(doc).toContain("<title>Edited Index header</title>");
    expect(doc).toContain('<main class="card">Hello</main>');
  });

  it("falls back to the canvas label when the message title is missing or blank", () => {
    for (const title of [undefined, null, "", "   "]) {
      const doc = buildStandaloneArtifactHtml("<main>Hi</main>", title);
      expect(doc).toContain(`<title>${ARTIFACT_EXPORT_FALLBACK_TITLE}</title>`);
    }
    expect(ARTIFACT_EXPORT_FALLBACK_TITLE).toBe("Generated artifact");
  });

  it("escapes title text instead of injecting markup", () => {
    // DOM serialization since 2026-09-11 (parser rewrite): `<`, `&` are
    // escaped, `"` stays raw in text nodes. Same document, equivalent bytes —
    // the old `&quot;` spelling was string-splicing, not the parser.
    const doc = buildStandaloneArtifactHtml("<main>Hi</main>", 'A <b> & "quoted" title');
    expect(doc).toContain('<title>A &lt;b&gt; &amp; "quoted" title</title>');
  });

  it("passes a complete document through without a second html element", () => {
    const fragment =
      '<!DOCTYPE html>\n<html lang="it">\n<head>\n<meta charset="utf-8">\n' +
      '<meta name="viewport" content="width=device-width, initial-scale=1">\n' +
      "<title>Modello</title>\n<style>p{color:red}</style>\n</head>\n" +
      "<body>\n<main>Corpo</main>\n</body>\n</html>";
    const doc = buildStandaloneArtifactHtml(fragment, "Edited Index header");

    expect(countOccurrences(doc.toLowerCase(), "<html")).toBe(1);
    expect(countOccurrences(doc.toLowerCase(), "</html>")).toBe(1);
    // The document declares itself: its own language and title win, nothing is doubled.
    expect(doc).toContain('<html lang="it">');
    expect(doc).toContain("<title>Modello</title>");
    expect(countOccurrences(doc.toLowerCase(), "<title")).toBe(1);
    expect(countOccurrences(doc, 'charset="utf-8"')).toBe(1);
    expect(doc).toContain("<main>Corpo</main>");
  });

  it("repairs a complete document that lacks doctype, lang, and head pieces", () => {
    const doc = buildStandaloneArtifactHtml(
      "<html>\n<head></head>\n<body><main>Hi</main></body>\n</html>",
      "Edited Index header",
    );

    expect(doc.startsWith("<!DOCTYPE html>\n")).toBe(true);
    expect(doc).toContain('<html lang="en">');
    expect(doc).toContain('<meta charset="utf-8">');
    expect(doc).toContain('<meta name="viewport"');
    expect(doc).toContain("<title>Edited Index header</title>");
    expect(doc).toContain("<main>Hi</main>");
  });

  it("creates a head when a complete document has none", () => {
    const doc = buildStandaloneArtifactHtml("<html><body><main>Hi</main></body></html>", "T");
    expect(doc).toContain("<head>");
    expect(doc).toContain('<meta charset="utf-8">');
    expect(doc).toContain("<title>T</title>");
    expect(countOccurrences(doc.toLowerCase(), "<head")).toBe(1);
  });

  it("never ships the app CSP meta, in either shape", () => {
    const wrapped = buildStandaloneArtifactHtml(`${APP_CSP_META}\n<main>Hi</main>`, "T");
    expect(wrapped.toLowerCase()).not.toContain("content-security-policy");

    const upgraded = buildStandaloneArtifactHtml(
      `<!DOCTYPE html><html><head>${APP_CSP_META}</head><body><main>Hi</main></body></html>`,
      "T",
    );
    expect(upgraded.toLowerCase()).not.toContain("content-security-policy");
    expect(upgraded).toContain("<main>Hi</main>");
  });

  it("leaves a body style block where the model wrote it", () => {
    // Hoisting removed 2026-09-11 (F3): the cascade applies document-wide,
    // so moving a style never changed rendering — it only rewrote the
    // source (emptied `<pre>`, duplicated blocks). The parser keeps
    // body-authored styles in the body, exactly like the canvas iframe does.
    const doc = buildStandaloneArtifactHtml(
      "<style>p{color:red}</style><main><p>Hi</p></main>",
      "T",
    );

    expect(countOccurrences(doc, "<style>p{color:red}</style>")).toBe(1);
    const head = doc.slice(0, doc.indexOf("</head>"));
    const body = doc.slice(doc.indexOf("<body>"));
    expect(head).not.toContain("<style>");
    expect(body).toContain("<style>p{color:red}</style>");
    expect(body).toContain("<p>Hi</p>");
  });

  it("keeps a partial head where the model wrote it and emits exactly one title", () => {
    // Order flipped 2026-09-11 (defect 2, live evidence): a run title is a
    // status ("Edited Index header", worse: "Agent did not report written
    // files"), not a name, and it shipped verbatim into the browser tab.
    // A page that named itself wins over the run that produced it.
    const withMessageTitle = buildStandaloneArtifactHtml(
      "<head><style>p{color:red}</style><title>Modello</title></head><main><p>Hi</p></main>",
      "Edited Index header",
    );
    expect(countOccurrences(withMessageTitle.toLowerCase(), "<title")).toBe(1);
    expect(withMessageTitle).toContain("<title>Modello</title>");
    expect(withMessageTitle.slice(0, withMessageTitle.indexOf("</head>"))).toContain(
      "<style>p{color:red}</style>",
    );

    const withoutMessageTitle = buildStandaloneArtifactHtml(
      "<head><title>Modello</title></head><main><p>Hi</p></main>",
    );
    expect(withoutMessageTitle).toContain("<title>Modello</title>");
  });

  it("prefers the page h1 over a diagnostic run title", () => {
    const doc = buildStandaloneArtifactHtml(
      "<main><h1>Our menu</h1><p>Hi</p></main>",
      "Agent did not report written files",
    );
    expect(doc).toContain("<title>Our menu</title>");
  });

  it("reads the h1 as text, not markup", () => {
    const doc = buildStandaloneArtifactHtml(
      "<main><h1>Our <em>seasonal</em>  menu</h1></main>",
      "Agent did not report written files",
    );
    expect(doc).toContain("<title>Our seasonal menu</title>");
  });

  it("bounds a runaway h1 instead of pasting it whole", () => {
    const long = `x${"y".repeat(200)}`;
    const doc = buildStandaloneArtifactHtml(`<main><h1>${long}</h1></main>`, "Run title");
    const title = doc.match(/<title>([\s\S]*?)<\/title>/)?.[1] ?? "";
    expect(title.length).toBeLessThanOrEqual(121);
    expect(title.endsWith("…")).toBe(true);
  });

  it("uses the h1 for a complete document that declares no title", () => {
    const doc = buildStandaloneArtifactHtml(
      "<html><head></head><body><main><h1>Our menu</h1></main></body></html>",
      "Agent did not report written files",
    );
    expect(countOccurrences(doc.toLowerCase(), "<title")).toBe(1);
    expect(doc).toContain("<title>Our menu</title>");
  });

  it("falls through a blank h1 to the run title", () => {
    const doc = buildStandaloneArtifactHtml("<main><h1>   </h1><p>Hi</p></main>", "Run title");
    expect(doc).toContain("<title>Run title</title>");
  });

  it("imposes no font styling the fragment did not declare", () => {
    const plain = buildStandaloneArtifactHtml("<main><p>Hi</p></main>", "T");
    expect(plain.toLowerCase()).not.toContain("font-");

    const declared = buildStandaloneArtifactHtml(
      "<style>p{font-family:system-ui}</style><main><p>Hi</p></main>",
      "T",
    );
    expect(declared).toContain("p{font-family:system-ui}");
  });

  it("still builds a valid shell for an empty fragment", () => {
    const doc = buildStandaloneArtifactHtml("", "T");
    expect(doc).toContain("<!DOCTYPE html>");
    expect(doc).toContain("<title>T</title>");
    expect(doc).toContain("<body>");
  });

  it("ignores an svg title when naming the page (F1)", () => {
    // `<title>` inside `<svg>` is accessibility text in another namespace,
    // not the document title. The regex read it as the page name, so every
    // page with an inline icon exported as "Icona freccia".
    const doc = buildStandaloneArtifactHtml(
      '<main><svg width="24"><title>Icona freccia</title></svg><h1>Dashboard</h1></main>',
    );
    expect(doc).toContain("<title>Dashboard</title>");
    expect(doc).toContain("<title>Icona freccia</title>");
  });

  it("merges a doubled head into a single title (F2)", () => {
    // The parser folds the second `<head>` into the body; the regex kept
    // both titles and leaked "Second" as visible page text.
    const doc = buildStandaloneArtifactHtml(
      "<head><title>First</title></head><head><title>Second</title></head><main>Hi</main>",
    );
    expect(countOccurrences(doc.toLowerCase(), "<title")).toBe(1);
    expect(doc).toContain("<title>First</title>");
    expect(doc).not.toContain("Second");
  });

  it("leaves a style inside pre exactly where the model wrote it (F3)", () => {
    // Hoisting ripped the style out of the `<pre>` and left it empty.
    // Rendering was identical either way (the cascade is document-wide),
    // which is precisely why moving it was pointless as well as harmful.
    const doc = buildStandaloneArtifactHtml(
      "<main><pre><style>not-real-css</style></pre></main>",
      "T",
    );
    expect(doc.indexOf("<style>")).toBeGreaterThan(doc.indexOf("</head>"));
    expect(doc).toContain("<pre><style>not-real-css</style></pre>");
  });

  it("never copies a style block the model already placed (F4)", () => {
    // The head style and the body style are two authored placements, not
    // two copies: the exporter moves nothing, so it cannot duplicate.
    const doc = buildStandaloneArtifactHtml(
      "<head><style>.x{color:blue}</style></head><style>.x{color:blue}</style><main>Hi</main>",
      "T",
    );
    const head = doc.slice(0, doc.indexOf("</head>"));
    expect(countOccurrences(head, "<style>.x{color:blue}</style>")).toBe(1);
    expect(countOccurrences(doc, "<style>.x{color:blue}</style>")).toBe(2);
  });

  it("does not read a title out of an HTML comment", () => {
    // Fifth case, same family: the regex matched `<title>` inside a comment
    // and crowned it the page name. The parser knows comments are not elements.
    const doc = buildStandaloneArtifactHtml(
      "<!-- <title>Fake</title> --><main><h1>Real</h1></main>",
    );
    expect(doc).toContain("<title>Real</title>");
  });

  it("keeps the first of two titles inside one head", () => {
    // F2's sibling: both titles survive parsing in the head, but the
    // browser only ever honors the first, so the export keeps just it.
    const doc = buildStandaloneArtifactHtml(
      "<head><title>A</title><title>B</title></head><main>Hi</main>",
    );
    expect(countOccurrences(doc.toLowerCase(), "<title")).toBe(1);
    expect(doc).toContain("<title>A</title>");
  });
});
