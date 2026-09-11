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
    const doc = buildStandaloneArtifactHtml("<main>Hi</main>", 'A <b> & "quoted" title');
    expect(doc).toContain("<title>A &lt;b&gt; &amp; &quot;quoted&quot; title</title>");
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

  it("hoists a fragment style block into the head instead of leaving it in the body", () => {
    const doc = buildStandaloneArtifactHtml(
      "<style>p{color:red}</style><main><p>Hi</p></main>",
      "T",
    );

    expect(countOccurrences(doc, "<style>p{color:red}</style>")).toBe(1);
    const head = doc.slice(0, doc.indexOf("</head>"));
    const body = doc.slice(doc.indexOf("<body>"));
    expect(head).toContain("<style>p{color:red}</style>");
    expect(body).not.toContain("<style>");
    expect(body).toContain("<p>Hi</p>");
  });

  it("hoists a partial head and emits exactly one title", () => {
    const withMessageTitle = buildStandaloneArtifactHtml(
      "<head><style>p{color:red}</style><title>Modello</title></head><main><p>Hi</p></main>",
      "Edited Index header",
    );
    expect(countOccurrences(withMessageTitle.toLowerCase(), "<title")).toBe(1);
    expect(withMessageTitle).toContain("<title>Edited Index header</title>");
    expect(withMessageTitle.slice(0, withMessageTitle.indexOf("</head>"))).toContain(
      "<style>p{color:red}</style>",
    );

    const withoutMessageTitle = buildStandaloneArtifactHtml(
      "<head><title>Modello</title></head><main><p>Hi</p></main>",
    );
    expect(withoutMessageTitle).toContain("<title>Modello</title>");
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
});
