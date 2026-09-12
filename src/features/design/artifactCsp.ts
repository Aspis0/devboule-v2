/**
 * The one confinement policy for a generated artifact, its `<meta>` line, and
 * the srcdoc that delivers that line.
 *
 * This module owns the policy and the string that carries it, and nothing else
 * — no React, no DOM, no other module of the Design feature — so the canvas
 * that renders under it and the export that writes it into a file depend on the
 * same source instead of on each other. The render critic derives its
 * near-identical policy from this base too (see `artifactRenderCritic.ts`).
 *
 * The canvas enforces the policy inside the artifact iframe, and the exported
 * `.html` carries the same one: the export asks the model for a
 * self-contained document (see `agentHost.ts`), so a compliant artifact has no
 * external reference for the strict directives to break. "What the user
 * previewed is what they saved" is then a consequence of one shared constant
 * rather than a coincidence two files have to keep true separately.
 */

export const ARTIFACT_CSP =
  "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; manifest-src 'none'";

export const ARTIFACT_CSP_META = `<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_CSP}" />`;

/**
 * The document an artifact is rendered from: the policy line, then the markup.
 *
 * Shared by the canvas and the side panel's scaled preview rather than copied
 * into each. The parent CSP is not inherited by srcdoc, so every frame has to
 * deliver the policy itself, and two copies of this string could drift: a
 * preview left behind on an older, weaker policy would be a hole disguised as a
 * convenience.
 */
export function artifactSrcDoc(html: string): string {
  return `${ARTIFACT_CSP_META}\n${html}`;
}
