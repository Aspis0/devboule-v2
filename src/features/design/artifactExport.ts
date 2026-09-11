/**
 * Standalone `.html` export for a generated artifact.
 *
 * What the surface renders is a *fragment*, not a document: DesignSurface
 * wraps it only in one CSP meta line for the canvas iframe (`artifactSrcDoc`).
 * This module turns that fragment into a document that shows the same thing
 * when opened in any browser outside the app. It is pure (no clipboard) and
 * runs wherever `DOMParser` exists — the WebView2 renderer and the happy-dom
 * test environment alike.
 *
 * The first version did this job with eight regular expressions and failed
 * four live inputs (2026-09-11, hostile review): an `<svg><title>` read as
 * the page name, a doubled `<head>` leaking two titles, a `<style>` ripped
 * out of a `<pre>`, one style block emitted twice. A regex cannot know what
 * a nested element is, so the fix is not a ninth regex: parse once with
 * `DOMParser`, normalize the tree (fill the gaps, remove the forbidden),
 * serialize. The canvas iframe parses the same fragment the same way, so
 * tree fidelity IS rendering fidelity.
 *
 * Deliberately, there is no "is this already a document?" pre-check on the
 * raw string: the parser normalizes fragments and documents into the same
 * shape (`html > head + body`), and every step below only adds missing
 * required pieces or removes forbidden ones. Any string-level branch here
 * would reintroduce exactly the fragility the parser removes. In
 * particular, nothing authored is ever moved: a style the model wrote in
 * the head stays in the head, one written in the body stays in the body —
 * the cascade applies document-wide either way, so hoisting was pointless
 * as well as harmful, and not moving means nothing can be duplicated.
 *
 * Two non-goals, both load-bearing for fidelity:
 *
 * - The app's confinement CSP is NEVER carried into the file, but the file
 *   is not left policy-less either. That policy (`default-src 'none'`,
 *   `font-src 'none'`, …) exists to confine the canvas iframe inside the
 *   app; on the user's disk there is no attacker for it to stop, and it
 *   would only break rendering (fonts, images, styles all fall under
 *   `default-src`). Every CSP meta the fragment carries, wherever parsed,
 *   is removed.
 *
 *   What is written instead is a single minimal `script-src 'none'`. The
 *   canvas delivers that same guarantee *inside* the frame
 *   (`artifactSrcDoc`), so without it the shown page and the delivered page
 *   would diverge: a `<script>` the model authored is inert in the preview
 *   and live the moment the saved file is opened in a real browser
 *   (measured on WebView2, 2026-09-05: the parent CSP is not inherited by a
 *   `srcdoc` frame, which is why the canvas injects it). The export states
 *   the one guarantee the surface itself keeps — scripts do not run — and
 *   nothing beyond it.
 * - No font or base styling is imposed (see module history: the canvas
 *   applies nothing from the outside into the frame, so whatever type the
 *   user sees is already declared by the fragment itself).
 */

export const ARTIFACT_EXPORT_FALLBACK_TITLE = "Generated artifact";

const EXPORT_LANG = "en";
const DOCTYPE_HTML = "<!DOCTYPE html>";
const XHTML_NS = "http://www.w3.org/1999/xhtml";
const MAX_H1_TITLE_CHARS = 120;

function nonBlank(value: string | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

/**
 * HTML-namespace `<title>` elements under `root`, in tree order. The
 * namespace check is the F1 fix: `querySelectorAll("title")` also matches
 * `<svg><title>` (same local name, other namespace), which is accessibility
 * text, not a page name — the parser knows the difference, a regex did not.
 */
function htmlTitlesUnder(root: ParentNode): Element[] {
  return [...root.querySelectorAll("title")].filter((el) => el.namespaceURI === XHTML_NS);
}

/**
 * The one policy the exported file carries, and the only one: scripts do not
 * run. Deliberately narrower than the canvas policy, which also names
 * fonts, images, styles and frames — those directives have no purpose on the
 * user's disk and would only break the page. No `default-src`, so a fragment
 * that loads a font, an image or a style keeps behaving as it did on screen.
 */
const EXPORT_CSP = "script-src 'none'";

function stripCspMetas(doc: Document): void {
  for (const meta of doc.querySelectorAll("meta[http-equiv]")) {
    if (meta.getAttribute("http-equiv")?.toLowerCase() === "content-security-policy") {
      meta.remove();
    }
  }
}

/**
 * Add the export's own script policy to `head`, after every model-authored
 * CSP meta has been stripped, so it is the only policy in the document. The
 * head always exists after parsing (`DOMParser` fabricates one for a bare
 * fragment), so a fragment that never wrote a head still receives it.
 */
function appendExportCspMeta(doc: Document, head: HTMLHeadElement): void {
  const meta = doc.createElement("meta");
  meta.setAttribute("http-equiv", "Content-Security-Policy");
  meta.setAttribute("content", EXPORT_CSP);
  head.appendChild(meta);
}

function headHasCharset(head: HTMLHeadElement): boolean {
  return head.querySelector("meta[charset]") !== null;
}

function headHasViewport(head: HTMLHeadElement): boolean {
  return [...head.querySelectorAll("meta[name]")].some(
    (meta) => meta.getAttribute("name")?.toLowerCase() === "viewport",
  );
}

function h1Text(doc: Document): string | null {
  const text = doc.body?.querySelector("h1")?.textContent;
  const collapsed = text?.replace(/\s+/g, " ").trim();
  if (!collapsed) return null;
  if (collapsed.length <= MAX_H1_TITLE_CHARS) return collapsed;
  return `${collapsed.slice(0, MAX_H1_TITLE_CHARS).trimEnd()}…`;
}

/**
 * Who names the exported page. The page speaks first — its head `<title>`,
 * then the text of its first `<h1>` — because both describe the page
 * itself. The run title is a status ("Edited Index header", worse: "Agent
 * did not report written files"), not a name; shown live it shipped verbatim
 * into the browser tab. It only speaks when the page is silent, and the
 * honest fallback closes the chain.
 */
function resolveExportTitle(doc: Document, explicitTitle: string | null): string {
  const declared = nonBlank(htmlTitlesUnder(doc.head)[0]?.textContent);
  return declared ?? h1Text(doc) ?? explicitTitle ?? ARTIFACT_EXPORT_FALLBACK_TITLE;
}

/**
 * Build the standalone document for `fragment`. `title` is the title of the
 * assistant message that produced the artifact, but it is only a fallback:
 * the page names itself first (its `<title>`, then its first `<h1>`).
 * Author bytes are preserved modulo parser normalization (entity and
 * attribute spelling may be reflowed; the tree is untouched).
 */
export function buildStandaloneArtifactHtml(fragment: string, title?: string | null): string {
  const doc = new DOMParser().parseFromString(fragment, "text/html");
  const explicit = nonBlank(title);
  stripCspMetas(doc);

  const html = doc.documentElement;
  if (!html.hasAttribute("lang")) html.setAttribute("lang", EXPORT_LANG);

  const head = doc.head;
  if (!headHasCharset(head)) {
    const meta = doc.createElement("meta");
    meta.setAttribute("charset", "utf-8");
    head.insertBefore(meta, head.firstChild);
  }
  if (!headHasViewport(head)) {
    const meta = doc.createElement("meta");
    meta.setAttribute("name", "viewport");
    meta.setAttribute("content", "width=device-width, initial-scale=1");
    head.appendChild(meta);
  }
  appendExportCspMeta(doc, head);

  const resolved = resolveExportTitle(doc, explicit);
  // Exactly one title, in the head. Strays (a folded second `<head>`, a
  // doubled head title) are removed by tree position — keep the head's
  // first, drop the rest — never by string comparison, so identical blocks
  // the model wrote twice in different places are each left alone.
  const keeper = htmlTitlesUnder(head)[0] ?? null;
  for (const stray of htmlTitlesUnder(doc)) {
    if (stray !== keeper) stray.remove();
  }
  if (keeper) {
    if (keeper.textContent !== resolved) keeper.textContent = resolved;
  } else {
    const titleEl = doc.createElement("title");
    titleEl.textContent = resolved;
    head.appendChild(titleEl);
  }

  return `${DOCTYPE_HTML}\n${html.outerHTML}\n`;
}
