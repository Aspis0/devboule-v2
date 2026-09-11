/**
 * The printed artifact: the document, the policy it prints under, and the
 * stylesheet that puts one slide on one page. Everything here is pure over the
 * artifact text and a `DOMParser`; the control that mounts the frame lives in
 * `ArtifactPrintControl.tsx`, and this split is the same one `artifactSlides.ts`
 * and `ArtifactSaveControl.tsx` already use, so the decisions can be pinned
 * without a renderer.
 *
 * WHY A SECOND FRAME AT ALL
 *
 * The canvas frame is `sandbox=""` with `script-src 'none'` (`artifactCsp.ts`),
 * so the user cannot reach into it and the artifact cannot call `window.print()`
 * on itself. The panel could import a PDF library and rebuild the page by hand,
 * or call into Rust; both were considered and rejected, because either one
 * re-renders the artifact with an engine that is not the one the user previewed.
 * The WebView's own print pipeline renders the document the canvas showed, and
 * that is the whole point: a print that disagrees with the preview is a bug the
 * user cannot see until the paper comes out.
 *
 * So this module builds a THIRD document: the standalone export (the same bytes
 * "Save HTML" writes and "Copy HTML" copies, via `buildStandaloneArtifactHtml`)
 * with a print stylesheet and one inline script appended, and the frame that
 * carries it is short-lived. Two consequences follow, and both are load-bearing:
 *
 * - The document may run an inline script, so the policy it carries cannot be
 *   `script-src 'none'`. It is derived from `ARTIFACT_CSP` by swapping exactly
 *   that one directive — the same derivation the render critic performs for its
 *   measurement frame (`artifactRenderCritic.ts`), and refused here when the
 *   base policy stops having the shape the swap assumes. A second hand-written
 *   policy is how the canvas and the export drift apart.
 * - Because that policy permits inline script generally, the artifact's own
 *   `<script>` elements and `on*` attributes are stripped before insertion, the
 *   same way the critic strips them. Nothing is lost: under the canvas policy
 *   those scripts never ran either, so the printed page still shows what the
 *   preview showed.
 */

import { ARTIFACT_CSP } from "./artifactCsp";
import { buildStandaloneArtifactHtml } from "./artifactExport";
import { stripArtifactScriptsAndHandlers } from "./artifactRenderCritic";
import { SLIDE_ID_PREFIX, readArtifactSlideShape, type ArtifactSlideShape } from "./artifactSlides";
import type { DesignOutputMode } from "./designHost";

/**
 * Who a print report comes from. Named so a message that merely looks like one
 * is not one, and so the frame and the validating reader cannot drift apart.
 */
export const ARTIFACT_PRINT_SOURCE = "devboule-artifact-print";
export const ARTIFACT_PRINT_MESSAGE_KIND = "artifact-print-report";
export const ARTIFACT_PRINT_VERSION = 1 as const;

/**
 * The print frame is sandboxed, and it is the only frame in this feature that
 * carries script permission.
 *
 * `allow-scripts` because the document has to call `window.print()`; the critic
 * grants it for the same kind of reason. `allow-modals` because `window.print()`
 * is gated by the sandboxed modals flag in the HTML specification — without it
 * the call is silently ignored in some engines and throws in others, which would
 * look like a broken button. Deliberately absent: `allow-same-origin` (the frame
 * is an untrusted document, so it must not share the app's origin, and without
 * it `allow-scripts` cannot be used to escape the sandbox), `allow-popups`,
 * `allow-top-navigation`, `allow-forms` and `allow-downloads`.
 */
export const ARTIFACT_PRINT_SANDBOX = "allow-scripts allow-modals";

/**
 * The sheet the print dialog opens on. A4 is the common default for both a
 * document and a deck, and the artifact is authored against a 1280 CSS px width
 * (`artifactViewport.ts`), so the browser refits it the same way it refits any
 * page; the orientation is what actually differs (see `readArtifactPrintLayout`).
 */
export const ARTIFACT_PRINT_PAGE_SIZE = "A4";

const ARTIFACT_PRINT_DOCTYPE = "<!DOCTYPE html>";

const ARTIFACT_PRINT_SCRIPT_DIRECTIVE = "script-src 'none'";
const ARTIFACT_PRINT_SCRIPT_DIRECTIVE_SWAP = "script-src 'unsafe-inline'";

/**
 * Thrown when the print policy cannot be shown to be the canvas policy with one
 * directive swapped, and when the exported document does not carry that policy
 * in exactly one meta tag. Named so a failed import says which invariant broke
 * instead of surfacing as an opaque module evaluation error.
 */
export class ArtifactPrintCspError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ArtifactPrintCspError";
  }
}

/**
 * The print policy: the canvas policy with exactly one directive swapped.
 *
 * `String.replace` returns its subject unchanged when the pattern does not
 * match, and replaces only the first match when it does, so the derivation is
 * only sound while `ARTIFACT_CSP` contains the source directive exactly once. A
 * renamed, reordered or duplicated directive would otherwise ship a policy that
 * bans the print trigger, and nothing would say so until a button that does
 * nothing. The failure is raised to the caller rather than swallowed, and the
 * module-scope constant below makes it a load-time failure.
 */
export function deriveArtifactPrintCsp(base: string): string {
  const occurrences = base.split(ARTIFACT_PRINT_SCRIPT_DIRECTIVE).length - 1;
  if (occurrences !== 1) {
    throw new ArtifactPrintCspError(
      `ARTIFACT_CSP must contain "${ARTIFACT_PRINT_SCRIPT_DIRECTIVE}" exactly once for the print document to derive its policy from it; it occurs ${occurrences} times. Fix artifactCsp.ts: a derivation that does not match returns the policy unchanged, banning the print trigger.`,
    );
  }
  const derived = base.replace(
    ARTIFACT_PRINT_SCRIPT_DIRECTIVE,
    ARTIFACT_PRINT_SCRIPT_DIRECTIVE_SWAP,
  );
  if (derived === base) {
    throw new ArtifactPrintCspError(
      `Deriving the print policy from ARTIFACT_CSP returned it unchanged: "${ARTIFACT_PRINT_SCRIPT_DIRECTIVE_SWAP}" was not substituted for "${ARTIFACT_PRINT_SCRIPT_DIRECTIVE}". Fix artifactCsp.ts.`,
    );
  }
  return derived;
}

export const ARTIFACT_PRINT_CSP = deriveArtifactPrintCsp(ARTIFACT_CSP);

/**
 * How the artifact paginates: a page per slide in landscape, or one continuous
 * sheet in portrait.
 *
 * THE DECISION, AND THE MEASURED DEFECT THAT RESHAPED IT
 *
 * This used to be decided by the shape alone, on `sectionCount > 0`. Measured
 * live on 2026-09-11: a pricing page — three plan cards, a FAQ, a footer, one
 * continuous document — is built from three `<section>` elements carrying no
 * ids, and it printed as three landscape sheets cut at boundaries its author
 * never drew. `<section>` is an ordinary HTML landmark; documentation pages,
 * pricing pages and articles all use it. "Has at least one section" was a proxy
 * for "is a deck", and the common case failed it.
 *
 * So the mode the producing run recorded decides, and it decides outright: it
 * is a fact about the artifact (`DesignOutputMode`, carried on the artifact by
 * the run that produced it), not a reading of its markup. A recorded `page` is
 * continuous portrait no matter how many sections the document contains, and a
 * recorded `slides` is a deck no matter how badly the ids are spelled. Reading
 * it off the artifact is the same rule the slides notice already follows, and
 * for the same reason: the toggle beside the canvas states what the *next* run
 * will ask for and regenerates nothing, so it cannot describe what is on screen.
 *
 * ABSENT MODE IS A THIRD STATE, NOT `page`
 *
 * An artifact reopened from design history records none, so the mode is
 * genuinely missing and the markup is the only evidence left. The fallback
 * asks the question the shape can actually answer — did this document attempt
 * the slides contract? — and the answer is whether any section carries a
 * `slide-N` id. That is read from the exported `SLIDE_ID_PREFIX` and the shape's
 * own `ids` list, which keeps `""` for a section with no usable id, so no new
 * field is needed to tell "some sections are numbered" from "none is".
 *
 * The rule is one predicate over the ids rather than a table over the verdicts,
 * because the verdicts are about the naming and this question is about the
 * attempt, and the two disagree in both directions:
 *
 * - `no-sections` — no ids, so a page. Nothing attempted the contract.
 * - `missing-ids` with none numbered (`["", "", ""]`) — the pricing page
 *   above, and a page. This is the measured case.
 * - `missing-ids` with some numbered (`["slide-1", "", ""]`) — a deck whose
 *   naming is incomplete, so a deck.
 * - `duplicate-ids` and `off-sequence` — a deck when an id carries the prefix
 *   (`["slide-1", "slide-1"]`, `["slide-2", "slide-1"]`) and a page when none
 *   does (`["cover", "agenda", "close"]`, a pricing page with `id="pricing"`
 *   and `id="faq"`). A verdict-only table would call the second group decks,
 *   which is the same defect one step narrower.
 * - `matches` — every id carries the prefix, so a deck, and the predicate agrees.
 *
 * Note what is deliberately NOT used here: the verdict alone. `artifactSlideNotice`
 * reports the naming contract and keeps doing exactly that; this module answers a
 * different question and does not restate that one.
 */
export interface ArtifactPrintLayout {
  /** True when the artifact prints as a page-per-slide deck. */
  readonly deck: boolean;
  /** `landscape` for a deck, `portrait` for a page. */
  readonly orientation: "landscape" | "portrait";
  /** The CSS `@page` size keyword; see `ARTIFACT_PRINT_PAGE_SIZE`. */
  readonly pageSize: string;
}

/**
 * Whether the document attempted the slides contract: at least one section id
 * carries `SLIDE_ID_PREFIX`. Used only when the producing run recorded no mode,
 * because a recorded mode is a fact about the artifact and outranks a reading of
 * its markup.
 */
function shapeReadsAsDeck(shape: ArtifactSlideShape): boolean {
  return shape.ids.some((id) => id.startsWith(SLIDE_ID_PREFIX));
}

/**
 * The deck/page decision. The recorded mode wins outright; absent falls back to
 * the shape; nothing else is consulted.
 */
export function readArtifactPrintLayout(
  shape: ArtifactSlideShape,
  outputMode?: DesignOutputMode,
): ArtifactPrintLayout {
  const deck = outputMode === undefined ? shapeReadsAsDeck(shape) : outputMode === "slides";
  return {
    deck,
    orientation: deck ? "landscape" : "portrait",
    pageSize: ARTIFACT_PRINT_PAGE_SIZE,
  };
}

/**
 * The print stylesheet: the product, and the reason a printed page stops looking
 * like a screenshot of a web page.
 *
 * `@page { margin: 0 }` because the artifact lays itself out — its own padding,
 * its own grid, and a canvas preview that has no paper margin around it either.
 * A default 1 cm margin would shrink the page box and shift every wrapping
 * decision away from what the user approved. `print-color-adjust: exact` (and
 * its `-webkit-` alias, still the one some engines read) because a browser
 * otherwise drops backgrounds to save ink, which silently removes the panels and
 * bands most artifacts are made of. `break-after: avoid` on headings so a
 * heading is never the last line of a page. On a deck, `break-before: page` on
 * every section plus `break-inside: avoid` so a slide starts its own page and is
 * never split across two.
 *
 * The first section is exempted from `break-before` on purpose: it is already at
 * the top of the first page, and a forced break at the very start of a document
 * is exactly the shape that produces a leading blank page in some engines. The
 * break exists to separate slides from each other, not from the paper.
 *
 * A page-mode artifact gets no `section` rules at all, and that now matters for
 * documents that DO have sections: a recorded `page` whose markup is full of
 * `<section>` landmarks — the measured pricing page — must flow continuously, so
 * the absence of these rules is the fix, not a leftover. Emitting dead selectors
 * would also invite a later reader to think pagination was decided twice.
 */
export function buildArtifactPrintCss(
  shape: ArtifactSlideShape,
  outputMode?: DesignOutputMode,
): string {
  const layout = readArtifactPrintLayout(shape, outputMode);
  const rules = [
    `@page {
  size: ${layout.pageSize} ${layout.orientation};
  margin: 0;
}`,
    `html {
  print-color-adjust: exact;
  -webkit-print-color-adjust: exact;
}`,
    `h1,
h2,
h3,
h4,
h5,
h6 {
  break-after: avoid;
  break-inside: avoid;
}`,
  ];
  if (layout.deck) {
    rules.push(`section {
  break-before: page;
  break-inside: avoid;
}`);
    rules.push(`section:first-of-type {
  break-before: auto;
}`);
  }
  return `${rules.join("\n\n")}\n`;
}

/**
 * What the frame reports back. `printed` means the print dialog closed, not that
 * a copy was printed: no engine exposes that, and the user may have cancelled.
 * The parent's label is worded for exactly this, and the status is not pretended
 * to be more than it is.
 */
export type ArtifactPrintStatus = "printed" | "failed";

export interface ArtifactPrintReport {
  readonly kind: typeof ARTIFACT_PRINT_MESSAGE_KIND;
  /** The engine's own words when the print call failed, bounded. Never shown raw. */
  readonly message?: string;
  readonly source: typeof ARTIFACT_PRINT_SOURCE;
  readonly status: ArtifactPrintStatus;
  readonly version: typeof ARTIFACT_PRINT_VERSION;
}

/** Longest failure detail kept from the frame; the rest is a tooltip, not a log. */
const MAX_PRINT_MESSAGE_CHARS = 160;

/**
 * Validates a message from the print frame. Same contract as the critic's reader
 * and for the same reason: the listener is on `window`, so anything in the app
 * can post an object that looks like a report, and the frame is the only sender
 * whose reports may drive state. A malformed report is dropped rather than
 * repaired; there is nothing to guess at.
 */
export function readArtifactPrintReport(value: unknown): ArtifactPrintReport | null {
  if (typeof value !== "object" || value === null) return null;
  const report = value as Record<string, unknown>;
  if (
    report.kind !== ARTIFACT_PRINT_MESSAGE_KIND ||
    report.source !== ARTIFACT_PRINT_SOURCE ||
    report.version !== ARTIFACT_PRINT_VERSION
  ) {
    return null;
  }
  if (report.status !== "printed" && report.status !== "failed") return null;
  const message = report.message;
  if (message === undefined) {
    return {
      kind: ARTIFACT_PRINT_MESSAGE_KIND,
      source: ARTIFACT_PRINT_SOURCE,
      status: report.status,
      version: ARTIFACT_PRINT_VERSION,
    };
  }
  if (
    typeof message !== "string" ||
    message.length === 0 ||
    message.length > MAX_PRINT_MESSAGE_CHARS
  ) {
    return null;
  }
  return {
    kind: ARTIFACT_PRINT_MESSAGE_KIND,
    message,
    source: ARTIFACT_PRINT_SOURCE,
    status: report.status,
    version: ARTIFACT_PRINT_VERSION,
  };
}

/**
 * The window check first, the shape check second: a report is only accepted when
 * it comes from the frame this control mounted, so a stray post from another
 * part of the app can never end a print that is still in flight.
 */
export function readArtifactPrintMessage(
  event: MessageEvent<unknown>,
  frameWindow: Window | null,
): ArtifactPrintReport | null {
  if (frameWindow === null || event.source !== frameWindow) return null;
  return readArtifactPrintReport(event.data);
}

/**
 * The one script in the document: wait for the resources, print, report.
 *
 * `afterprint` is what "the dialog closed" means — it fires when the user
 * printed and when they cancelled, and there is no API that tells the two apart.
 * It is registered before `print()` because the event can fire synchronously
 * inside the call. `print()` itself throws when the sandbox or the WebView
 * refuses, and that throw is reported instead of being swallowed, so the control
 * can say "Print failed." rather than leaving a dead button.
 *
 * There is deliberately no timer that tears the frame down after a while. Such a
 * timer can only fire while the print flow is not blocking the renderer — which
 * is precisely when the dialog may still be open and the frame still needed.
 * The parent removes the frame on a report or on unmount, and both of those
 * happen when no dialog is waiting on the document.
 */
const ARTIFACT_PRINT_SCRIPT = `(() => {
  const SOURCE = "${ARTIFACT_PRINT_SOURCE}";
  const KIND = "${ARTIFACT_PRINT_MESSAGE_KIND}";
  const VERSION = ${ARTIFACT_PRINT_VERSION};

  function report(status, message) {
    const payload = { kind: KIND, source: SOURCE, version: VERSION, status: status };
    if (typeof message === "string" && message.length > 0) {
      payload.message = message.slice(0, ${MAX_PRINT_MESSAGE_CHARS});
    }
    try {
      window.parent.postMessage(payload, "*");
    } catch {
      // A report that cannot be sent is a frame the parent removes on unmount.
    }
  }

  let settled = false;
  function finish(status, message) {
    if (settled) return;
    settled = true;
    report(status, message);
  }

  function reason(cause) {
    if (cause && typeof cause.message === "string" && cause.message.length > 0) {
      return cause.message;
    }
    return "The print dialog could not be opened.";
  }

  function trigger() {
    if (typeof window.print !== "function") {
      finish("failed", "This WebView does not implement printing.");
      return;
    }
    window.addEventListener("afterprint", () => finish("printed"), { once: true });
    try {
      window.print();
    } catch (cause) {
      finish("failed", reason(cause));
    }
  }

  if (document.readyState === "complete") {
    window.setTimeout(trigger, 0);
  } else {
    window.addEventListener("load", () => window.setTimeout(trigger, 0), { once: true });
  }
})();`;

/**
 * Replaces the canvas policy the export carries with the print policy.
 *
 * `buildStandaloneArtifactHtml` strips every CSP meta the model wrote and adds
 * exactly one, whose content is `ARTIFACT_CSP`; that single meta is the one
 * changed here. Appending a second meta instead would not work, because CSP
 * policies are additive and the most restrictive wins — the canvas's
 * `script-src 'none'` would still be in force and the print trigger would never
 * run. The count check fails loudly for the same reason the derivation does: if
 * the export ever stops carrying exactly one policy meta, this module would
 * otherwise print a document under a policy nobody chose.
 */
function applyPrintPolicy(document: Document): void {
  const exported = [...document.querySelectorAll("meta[http-equiv]")].filter(
    (meta) => meta.getAttribute("content") === ARTIFACT_CSP,
  );
  const [policyMeta, ...duplicates] = exported;
  if (policyMeta === undefined || duplicates.length > 0) {
    throw new ArtifactPrintCspError(
      `The standalone export must carry the canvas policy (ARTIFACT_CSP) in exactly one meta tag for the print document to replace it; it carries ${exported.length}. Fix artifactExport.ts.`,
    );
  }
  policyMeta.setAttribute("content", ARTIFACT_PRINT_CSP);
}

/**
 * The complete document the print frame loads: the standalone export, printed
 * under the derived policy, with the print stylesheet and the trigger.
 *
 * The artifact is stripped of scripts and `on*` handlers first, because the
 * policy this document runs under permits inline script — the artifact's own
 * scripts would otherwise execute for the first time at print. The shape is read
 * from the exported document (not from the caller's fragment), so one-slide-per-
 * page and the orientation describe the bytes that actually print; the recorded
 * mode is passed through untouched and decides deck-vs-page before the shape is
 * consulted at all.
 */
export function buildArtifactPrintDocument(
  fragment: string,
  title?: string | null,
  outputMode?: DesignOutputMode,
): string {
  const standalone = buildStandaloneArtifactHtml(stripArtifactScriptsAndHandlers(fragment), title);
  const shape = readArtifactSlideShape(standalone);
  const document = new DOMParser().parseFromString(standalone, "text/html");
  applyPrintPolicy(document);

  const style = document.createElement("style");
  style.textContent = buildArtifactPrintCss(shape, outputMode);
  document.head.append(style);

  const script = document.createElement("script");
  script.textContent = ARTIFACT_PRINT_SCRIPT;
  document.body.append(script);

  return `${ARTIFACT_PRINT_DOCTYPE}\n${document.documentElement.outerHTML}\n`;
}
