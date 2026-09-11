/**
 * Standalone `.html` export for a generated artifact.
 *
 * What the surface renders is a *fragment*, not a document: DesignSurface
 * wraps it only in one CSP meta line for the canvas iframe (`artifactSrcDoc`).
 * This module turns that fragment into a document that shows the same thing
 * when opened in any browser outside the app. It is pure (no DOM, no
 * clipboard) so the export stays usable from both the UI action and tests.
 *
 * Two deliberate non-goals, both load-bearing for fidelity:
 *
 * - The app CSP meta is NEVER carried into the file. It exists to confine
 *   the canvas iframe inside the app (`default-src 'none'`, `font-src
 *   'none'`, …); on the user's disk that confinement has no attacker to
 *   stop and would only break rendering (no webfonts, no images except
 *   data: URLs). Any CSP meta found in the fragment is stripped.
 * - No font or base styling is imposed. The canvas applies nothing from the
 *   outside into the frame: parent-page CSS cannot cross a srcdoc boundary,
 *   and `.design-artifact-frame` only sizes the iframe box (width/height/
 *   border), never its content. The app CSP additionally blocks webfonts
 *   (`font-src 'none'`), so whatever type the user sees is already declared
 *   by the fragment itself (inline `<style>`, inline `style=` attributes,
 *   system fonts). Adding the app's fonts here would render something the
 *   user never saw; a fragment that declares nothing keeps declaring
 *   nothing and the browser falls back exactly as it did on canvas.
 */

export const ARTIFACT_EXPORT_FALLBACK_TITLE = "Generated artifact";

const EXPORT_LANG = "en";
const META_CHARSET = '<meta charset="utf-8">';
const META_VIEWPORT = '<meta name="viewport" content="width=device-width, initial-scale=1">';

const DOCTYPE_HTML = "<!DOCTYPE html>";

const CSP_META_RE = /<meta\b(?=[^>]*http-equiv)(?=[^>]*content-security-policy)[^>]*>/gi;
const DOCTYPE_RE = /<!doctype\b[^>]*>/i;
const HTML_OPEN_RE = /<html\b[^>]*>/i;
const LANG_ATTR_RE = /\blang\s*=/i;
const HEAD_OPEN_RE = /<head\b[^>]*>/i;
const HEAD_BLOCK_RE = /<head\b[^>]*>([\s\S]*?)<\/head\s*>/i;
const META_CHARSET_RE = /<meta\b[^>]*charset/i;
const META_VIEWPORT_RE = /<meta\b[^>]*name\s*=\s*("viewport"|'viewport'|viewport(?=[\s/>]))/i;
const TITLE_BLOCK_RE = /<title\b[^>]*>([\s\S]*?)<\/title\s*>/i;
const STYLE_BLOCK_RE = /<style\b[^>]*>[\s\S]*?<\/style\s*>/gi;

function escapeTitleText(title: string): string {
  return title
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

function nonBlank(value: string | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const trimmed = value.trim();
  return trimmed.length > 0 ? trimmed : null;
}

/**
 * A fragment that already carries a full document skeleton (`<html>`) is
 * patched, never re-wrapped: an `<html>` inside an `<html>` is a broken
 * file, and re-wrapping would also move the body the model wrote. Only the
 * pieces a standalone document needs and the fragment lacks are added
 * (doctype, `lang`, charset/viewport/title in `<head>`); every other byte
 * of the fragment is preserved so the page cannot change shape.
 */
function upgradeDocument(source: string, explicitTitle: string | null): string {
  let next = source;
  if (!DOCTYPE_RE.test(next)) next = `${DOCTYPE_HTML}\n${next}`;
  const htmlOpen = next.match(HTML_OPEN_RE)?.[0] ?? "";
  if (!LANG_ATTR_RE.test(htmlOpen)) {
    next = next.replace(HTML_OPEN_RE, (open) =>
      open.replace(/<html/i, `<html lang="${EXPORT_LANG}"`),
    );
  }
  const headOpen = next.match(HEAD_OPEN_RE)?.[0];
  if (headOpen === undefined) {
    const resolved = explicitTitle ?? declaredTitle(next) ?? ARTIFACT_EXPORT_FALLBACK_TITLE;
    return next.replace(
      HTML_OPEN_RE,
      (open) =>
        `${open}\n<head>\n${META_CHARSET}\n${META_VIEWPORT}\n<title>${escapeTitleText(resolved)}</title>\n</head>`,
    );
  }
  const additions: string[] = [];
  if (!META_CHARSET_RE.test(next)) additions.push(META_CHARSET);
  if (!META_VIEWPORT_RE.test(next)) additions.push(META_VIEWPORT);
  if (!TITLE_BLOCK_RE.test(next)) {
    const resolved = explicitTitle ?? ARTIFACT_EXPORT_FALLBACK_TITLE;
    additions.push(`<title>${escapeTitleText(resolved)}</title>`);
  }
  if (additions.length === 0) return next;
  return next.replace(HEAD_OPEN_RE, (open) => `${open}\n${additions.join("\n")}\n`);
}

function declaredTitle(source: string): string | null {
  return nonBlank(source.match(TITLE_BLOCK_RE)?.[1]);
}

/**
 * A body fragment is wrapped in a minimal shell. `<style>` blocks and a
 * partial `<head>` travel into the built `<head>` instead of staying in the
 * body: CSS applies document-wide wherever the block sits (there is no
 * scoping), so hoisting keeps every rule applying exactly as before while
 * the body keeps only markup. Exactly one `<title>` is emitted.
 *
 * Stray `<body>` tags without an `<html>` ancestor are left alone on
 * purpose: the HTML parser ignores a second body start tag and a stray
 * body end tag, so unwrapping them would only risk dropping real content
 * for zero rendering difference.
 */
function wrapFragment(source: string, explicitTitle: string | null): string {
  let body = source;
  let headExtras = "";
  let declared: string | null = null;
  const headBlock = body.match(HEAD_BLOCK_RE);
  if (headBlock !== null && headBlock[0] !== undefined && headBlock[1] !== undefined) {
    declared = nonBlank(headBlock[1].match(TITLE_BLOCK_RE)?.[1]);
    const rest = headBlock[1].replace(TITLE_BLOCK_RE, "").trim();
    if (rest.length > 0) headExtras += `${rest}\n`;
    body = body.replace(headBlock[0], "");
  }
  const styles = body.match(STYLE_BLOCK_RE) ?? [];
  for (const style of styles) body = body.replace(style, "");
  if (styles.length > 0) headExtras += `${styles.join("\n")}\n`;
  const resolved = explicitTitle ?? declared ?? ARTIFACT_EXPORT_FALLBACK_TITLE;
  return (
    `${DOCTYPE_HTML}\n` +
    `<html lang="${EXPORT_LANG}">\n` +
    `<head>\n${META_CHARSET}\n${META_VIEWPORT}\n<title>${escapeTitleText(resolved)}</title>\n` +
    `${headExtras}</head>\n` +
    `<body>\n${body.trim()}\n</body>\n` +
    `</html>\n`
  );
}

/**
 * Build the standalone document for `fragment`. `title` is the title of the
 * assistant message that produced the artifact; when it is missing or blank
 * the fragment's own `<title>` is used, and only then the honest fallback
 * ("Generated artifact", the same label the canvas frame already carries).
 * No markup is ever invented beyond the shell: the body is the fragment.
 */
export function buildStandaloneArtifactHtml(fragment: string, title?: string | null): string {
  const source = fragment.replace(CSP_META_RE, "");
  const explicit = nonBlank(title);
  if (HTML_OPEN_RE.test(source)) return upgradeDocument(source, explicit);
  return wrapFragment(source, explicit);
}
