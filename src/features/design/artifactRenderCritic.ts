import { createElement, useEffect, useState } from "react";

/*
 * This is a small render-time heuristic over an untrusted artifact, not an accessibility
 * audit. It measures only three things: text contrast, pointer-target size, and horizontal
 * clipping/overflow. It intentionally skips text with no measurable box, ambiguous colors,
 * background images or gradients, opacity/blending it cannot resolve, pseudo-elements,
 * replaced content, and targets it cannot identify as interactive (including custom interactions
 * exposed only through on* handlers, which are removed before measurement). It does not evaluate the
 * WCAG pointer-target exceptions for spacing, an equivalent control elsewhere, unmodified
 * user-agent controls, or essential presentation; native controls without an explicit ARIA
 * role are conservatively skipped, and inline targets in a run of text are skipped.
 * It also does not catch vertical clipping, overflow hidden without a wider scroll box,
 * transforms or clip-path that hide content, text rendered by canvas, shadow DOM, or defects
 * caused by a positioned sibling covering text, or defects that appear only after asynchronous
 * fonts, images, or other application state arrive.
 * These blind spots are deliberate: an ambiguous case is omitted rather than reported as a
 * verdict. The frame strips scripts and on* attributes before insertion, then posts only a
 * validated, bounded result to its parent; a timeout is treated as no result.
 */

export const ARTIFACT_RENDER_CRITIC_SOURCE = "devboule-artifact-render-critic";
export const ARTIFACT_RENDER_CRITIC_MESSAGE_KIND = "artifact-render-critic-result";
export const ARTIFACT_RENDER_CRITIC_VERSION = 1 as const;
export const ARTIFACT_RENDER_CRITIC_TIMEOUT_MS = 1_500;
export const ARTIFACT_RENDER_CRITIC_SANDBOX = "allow-scripts";
export const ARTIFACT_RENDER_CRITIC_CSP =
  "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; manifest-src 'none'";
export const ARTIFACT_RENDER_CRITIC_CSP_META = `<meta http-equiv="Content-Security-Policy" content="${ARTIFACT_RENDER_CRITIC_CSP}" />`;

interface ParsedTag {
  closing: boolean;
  name: string;
  nameEnd: number;
}

const RAW_TEXT_ELEMENTS = new Set(["style", "textarea", "title", "xmp", "noembed", "noframes"]);

function tagEnd(source: string, start: number): number {
  let quote: '"' | "'" | null = null;
  for (let index = start + 1; index < source.length; index += 1) {
    const character = source[index];
    if (quote !== null) {
      if (character === quote) quote = null;
    } else if (character === '"' || character === "'") {
      quote = character;
    } else if (character === ">") {
      return index;
    }
  }
  return -1;
}

function parseTag(source: string, start: number): ParsedTag | null {
  let index = start + 1;
  while (/\s/.test(source[index] ?? "")) index += 1;
  const closing = source[index] === "/";
  if (closing) index += 1;
  if (!/[A-Za-z]/.test(source[index] ?? "")) return null;
  const nameStart = index;
  while (/[A-Za-z0-9:-]/.test(source[index] ?? "")) index += 1;
  return {
    closing,
    name: source.slice(nameStart, index).toLowerCase(),
    nameEnd: index,
  };
}

function skipAttributeValue(source: string, start: number): number {
  let index = start;
  while (/\s/.test(source[index] ?? "")) index += 1;
  const quote = source[index];
  if (quote === '"' || quote === "'") {
    index += 1;
    while (index < source.length && source[index] !== quote) index += 1;
    return Math.min(source.length, index + 1);
  }
  while (index < source.length && !/[\s>]/.test(source[index]!)) index += 1;
  return index;
}

function stripEventHandlerAttributes(tag: string, parsed: ParsedTag): string {
  if (parsed.closing || tag.startsWith("<!") || tag.startsWith("<?")) return tag;

  let output = tag.slice(0, parsed.nameEnd);
  let index = parsed.nameEnd;
  while (index < tag.length) {
    const attributeStart = index;
    while (/\s/.test(tag[index] ?? "")) index += 1;
    if (index >= tag.length) break;
    if (tag[index] === ">") {
      output += tag.slice(attributeStart);
      break;
    }
    if (tag[index] === "/") {
      output += tag.slice(attributeStart);
      break;
    }

    const nameStart = index;
    while (!/[\s=>/]/.test(tag[index] ?? "")) index += 1;
    const attributeName = tag.slice(nameStart, index);
    while (/\s/.test(tag[index] ?? "")) index += 1;
    if (tag[index] === "=") index = skipAttributeValue(tag, index + 1);

    if (!/^on/i.test(attributeName)) {
      output += tag.slice(attributeStart, index);
    }
  }
  return output;
}

/** Remove script elements and every on* event-handler attribute without executing the markup. */
export function stripArtifactScriptsAndHandlers(html: string): string {
  let output = "";
  let cursor = 0;
  let rawTextElement: string | null = null;

  while (cursor < html.length) {
    if (rawTextElement !== null) {
      const closingRawText = new RegExp(`</${rawTextElement}\\s*>`, "gi");
      closingRawText.lastIndex = cursor;
      const match = closingRawText.exec(html);
      if (match === null) {
        output += html.slice(cursor);
        break;
      }
      const end = tagEnd(html, match.index);
      output += html.slice(cursor, end < 0 ? html.length : end + 1);
      cursor = end < 0 ? html.length : end + 1;
      rawTextElement = null;
      continue;
    }

    const start = html.indexOf("<", cursor);
    if (start < 0) {
      output += html.slice(cursor);
      break;
    }
    output += html.slice(cursor, start);

    if (html.startsWith("<!--", start)) {
      const commentEnd = html.indexOf("-->", start + 4);
      const end = commentEnd < 0 ? html.length : commentEnd + 3;
      output += html.slice(start, end);
      cursor = end;
      continue;
    }

    const end = tagEnd(html, start);
    const parsed = parseTag(html, start);
    if (end < 0) {
      if (parsed?.name === "script") break;
      const tag = html.slice(start);
      output += parsed === null ? tag : stripEventHandlerAttributes(tag, parseTag(tag, 0)!);
      break;
    }

    if (parsed?.name === "script") {
      if (parsed.closing) {
        cursor = end + 1;
        continue;
      }
      const closingScript = /<\/script\s*>/gi;
      closingScript.lastIndex = end + 1;
      const closingMatch = closingScript.exec(html);
      if (closingMatch === null) break;
      const closingEnd = tagEnd(html, closingMatch.index);
      cursor = closingEnd < 0 ? html.length : closingEnd + 1;
      continue;
    }

    const tag = html.slice(start, end + 1);
    output += parsed === null ? tag : stripEventHandlerAttributes(tag, parseTag(tag, 0)!);
    if (parsed !== null && !parsed.closing && RAW_TEXT_ELEMENTS.has(parsed.name)) {
      rawTextElement = parsed.name;
    }
    cursor = end + 1;
  }

  return output;
}

interface TagPosition {
  end: number;
  start: number;
}

function findTagPosition(source: string, name: string, closing: boolean): TagPosition | null {
  let cursor = 0;
  let rawTextElement: string | null = null;

  while (cursor < source.length) {
    if (rawTextElement !== null) {
      const closingRawText = new RegExp(`</${rawTextElement}\\s*>`, "i");
      closingRawText.lastIndex = cursor;
      const match = closingRawText.exec(source);
      if (match === null) return null;
      const end = tagEnd(source, match.index);
      if (end < 0) return null;
      cursor = end + 1;
      rawTextElement = null;
      continue;
    }

    const start = source.indexOf("<", cursor);
    if (start < 0) return null;
    if (source.startsWith("<!--", start)) {
      const commentEnd = source.indexOf("-->", start + 4);
      cursor = commentEnd < 0 ? source.length : commentEnd + 3;
      continue;
    }

    const end = tagEnd(source, start);
    if (end < 0) return null;
    const parsed = parseTag(source, start);
    if (parsed?.name === name && parsed.closing === closing) return { start, end };
    if (parsed !== null && !parsed.closing && RAW_TEXT_ELEMENTS.has(parsed.name)) {
      rawTextElement = parsed.name;
    }
    cursor = end + 1;
  }

  return null;
}

function preserveLeadingDoctype(source: string): { source: string; end: number } {
  const match = /^\s*(<!doctype\b[^>]*>)/i.exec(source);
  if (match === null) return { source, end: 0 };
  const doctype = match[1]!;
  return { source: `${doctype}${source.slice(match[0].length)}`, end: doctype.length };
}

function applyInsertions(
  source: string,
  insertions: readonly { at: number; text: string }[],
): string {
  return [...insertions]
    .sort((left, right) => right.at - left.at)
    .reduce(
      (current, insertion) =>
        `${current.slice(0, insertion.at)}${insertion.text}${current.slice(insertion.at)}`,
      source,
    );
}

function measurementScriptElement(): string {
  return `<script>${MEASUREMENT_SCRIPT}</script>`;
}

export function buildArtifactMeasurementSrcDoc(html: string): string {
  const preserved = preserveLeadingDoctype(stripArtifactScriptsAndHandlers(html));
  const source = preserved.source;
  const headOpen = findTagPosition(source, "head", false);
  const headClose = findTagPosition(source, "head", true);
  const bodyClose = findTagPosition(source, "body", true);
  const htmlOpen = findTagPosition(source, "html", false);
  const htmlClose = findTagPosition(source, "html", true);
  const insertions: { at: number; text: string }[] = [];

  if (headOpen !== null) {
    insertions.push({ at: headOpen.end + 1, text: `\n${ARTIFACT_RENDER_CRITIC_CSP_META}` });
  } else if (htmlOpen !== null) {
    insertions.push({
      at: htmlOpen.end + 1,
      text: `\n<head>\n${ARTIFACT_RENDER_CRITIC_CSP_META}\n</head>`,
    });
  } else {
    insertions.push({
      at: preserved.end,
      text: `${preserved.end === 0 ? "" : "\n"}<head>\n${ARTIFACT_RENDER_CRITIC_CSP_META}\n</head>`,
    });
  }

  const scriptAt = bodyClose?.start ?? headClose?.start ?? htmlClose?.start ?? source.length;
  insertions.push({ at: scriptAt, text: `\n${measurementScriptElement()}\n` });
  return applyInsertions(source, insertions);
}

export interface RgbColor {
  readonly b: number;
  readonly g: number;
  readonly r: number;
}

function linearChannel(channel: number): number {
  const normalized = channel / 255;
  return normalized <= 0.04045 ? normalized / 12.92 : Math.pow((normalized + 0.055) / 1.055, 2.4);
}

export function relativeLuminance(color: RgbColor): number {
  return (
    0.2126 * linearChannel(color.r) +
    0.7152 * linearChannel(color.g) +
    0.0722 * linearChannel(color.b)
  );
}

export function contrastRatio(foreground: RgbColor, background: RgbColor): number {
  const foregroundLuminance = relativeLuminance(foreground);
  const backgroundLuminance = relativeLuminance(background);
  const lighter = Math.max(foregroundLuminance, backgroundLuminance);
  const darker = Math.min(foregroundLuminance, backgroundLuminance);
  return (lighter + 0.05) / (darker + 0.05);
}

export function isLargeScaleText(fontSizePx: number, fontWeight: string | number): boolean {
  const numericWeight =
    typeof fontWeight === "number"
      ? fontWeight
      : fontWeight.toLowerCase() === "bold"
        ? 700
        : Number.parseInt(fontWeight, 10);
  return fontSizePx >= 24 || (fontSizePx >= 18.6666666667 && numericWeight >= 700);
}

export type ArtifactRenderFinding =
  | {
      kind: "contrast";
      count: number;
      samples: readonly ContrastSample[];
    }
  | {
      kind: "pointer-target";
      count: number;
      samples: readonly PointerTargetSample[];
    }
  | {
      kind: "overflow";
      count: number;
      samples: readonly OverflowSample[];
    };

export interface ContrastSample {
  readonly fontSizePx: number;
  readonly label: string;
  readonly minimum: number;
  readonly ratio: number;
}

export interface PointerTargetSample {
  readonly height: number;
  readonly label: string;
  readonly width: number;
}

export interface OverflowSample {
  readonly clientWidth: number;
  readonly label: string;
  readonly scrollWidth: number;
}

export interface ArtifactRenderCriticResult {
  readonly findings: readonly ArtifactRenderFinding[];
  readonly kind: typeof ARTIFACT_RENDER_CRITIC_MESSAGE_KIND;
  readonly source: typeof ARTIFACT_RENDER_CRITIC_SOURCE;
  readonly version: typeof ARTIFACT_RENDER_CRITIC_VERSION;
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : null;
}

function boundedNumber(value: unknown, maximum: number): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 && value <= maximum;
}

function boundedLabel(value: unknown): value is string {
  return typeof value === "string" && value.length > 0 && value.length <= 160;
}

function validSamples(value: unknown, count: number, kind: ArtifactRenderFinding["kind"]): boolean {
  if (!Array.isArray(value) || value.length === 0 || value.length > 3 || value.length > count) {
    return false;
  }
  return value.every((sampleValue) => {
    const sample = record(sampleValue);
    if (sample === null || !boundedLabel(sample.label)) return false;
    if (kind === "contrast") {
      return (
        boundedNumber(sample.ratio, 1000) &&
        sample.ratio > 0 &&
        (sample.minimum === 3 || sample.minimum === 4.5) &&
        boundedNumber(sample.fontSizePx, 10000) &&
        sample.fontSizePx > 0
      );
    }
    if (kind === "pointer-target") {
      return (
        boundedNumber(sample.width, 100000) &&
        sample.width > 0 &&
        boundedNumber(sample.height, 100000) &&
        sample.height > 0
      );
    }
    return (
      boundedNumber(sample.scrollWidth, 1000000) &&
      boundedNumber(sample.clientWidth, 1000000) &&
      sample.scrollWidth > sample.clientWidth
    );
  });
}

export function readArtifactRenderCriticResult(value: unknown): ArtifactRenderCriticResult | null {
  const message = record(value);
  if (
    message === null ||
    message.kind !== ARTIFACT_RENDER_CRITIC_MESSAGE_KIND ||
    message.source !== ARTIFACT_RENDER_CRITIC_SOURCE ||
    message.version !== ARTIFACT_RENDER_CRITIC_VERSION ||
    !Array.isArray(message.findings) ||
    message.findings.length > 3
  ) {
    return null;
  }

  const kinds = new Set<ArtifactRenderFinding["kind"]>();
  const findings: ArtifactRenderFinding[] = [];
  for (const findingValue of message.findings) {
    const finding = record(findingValue);
    if (
      finding === null ||
      typeof finding.kind !== "string" ||
      kinds.has(finding.kind as ArtifactRenderFinding["kind"])
    ) {
      return null;
    }
    if (
      finding.kind !== "contrast" &&
      finding.kind !== "pointer-target" &&
      finding.kind !== "overflow"
    ) {
      return null;
    }
    if (
      typeof finding.count !== "number" ||
      !Number.isInteger(finding.count) ||
      finding.count < 1 ||
      finding.count > 10000 ||
      !validSamples(finding.samples, finding.count, finding.kind)
    ) {
      return null;
    }
    kinds.add(finding.kind);
    findings.push(finding as ArtifactRenderFinding);
  }

  return {
    findings,
    kind: ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
    source: ARTIFACT_RENDER_CRITIC_SOURCE,
    version: ARTIFACT_RENDER_CRITIC_VERSION,
  };
}

export function readArtifactRenderCriticMessage(
  event: MessageEvent<unknown>,
  frameWindow: Window | null,
): ArtifactRenderCriticResult | null {
  if (event.source !== frameWindow) return null;
  return readArtifactRenderCriticResult(event.data);
}

const MEASUREMENT_SCRIPT = String.raw`(() => {
  const SOURCE = "${ARTIFACT_RENDER_CRITIC_SOURCE}";
  const KIND = "${ARTIFACT_RENDER_CRITIC_MESSAGE_KIND}";
  const SAMPLE_LIMIT = 3;

  function parseChannel(value) {
    const text = String(value).trim();
    const parsed = text.endsWith('%') ? Number.parseFloat(text) * 2.55 : Number.parseFloat(text);
    return Number.isFinite(parsed) ? Math.max(0, Math.min(255, parsed)) : null;
  }

  function parseAlpha(value) {
    const text = String(value).trim();
    const parsed = text.endsWith('%') ? Number.parseFloat(text) / 100 : Number.parseFloat(text);
    return Number.isFinite(parsed) ? Math.max(0, Math.min(1, parsed)) : null;
  }

  function parseColor(value) {
    const text = String(value || '').trim().toLowerCase();
    if (!text || text === 'transparent') return { r: 0, g: 0, b: 0, a: 0 };
    if (text.startsWith('#')) {
      const hex = text.slice(1);
      if (![3, 4, 6, 8].includes(hex.length) || !/^[0-9a-f]+$/.test(hex)) return null;
      const expandedHex = hex.length < 5 ? [...hex].map((part) => part + part).join("") : hex;
      const parts = expandedHex.match(/../g);
      if (!parts) return null;
      const values = parts.map((part) => Number.parseInt(part, 16));
      return {
        r: values[0],
        g: values[1],
        b: values[2],
        a: values.length === 4 ? values[3] / 255 : 1,
      };
    }
    const match = text.match(/^rgba?\((.*)\)$/);
    if (!match) return null;
    const parts = match[1].replace('/', ',').split(/\s*,\s*|\s+/).filter(Boolean);
    if (parts.length < 3) return null;
    const r = parseChannel(parts[0]);
    const g = parseChannel(parts[1]);
    const b = parseChannel(parts[2]);
    const a = parts.length > 3 ? parseAlpha(parts[3]) : 1;
    return r === null || g === null || b === null || a === null ? null : { r, g, b, a };
  }

  function composite(top, bottom) {
    return {
      r: top.r * top.a + bottom.r * (1 - top.a),
      g: top.g * top.a + bottom.g * (1 - top.a),
      b: top.b * top.a + bottom.b * (1 - top.a),
      a: top.a + bottom.a * (1 - top.a),
    };
  }

  function channel(value) {
    const normalized = value / 255;
    return normalized <= 0.04045
      ? normalized / 12.92
      : Math.pow((normalized + 0.055) / 1.055, 2.4);
  }

  function ratio(foreground, background) {
    const foregroundLuminance = 0.2126 * channel(foreground.r) + 0.7152 * channel(foreground.g) + 0.0722 * channel(foreground.b);
    const backgroundLuminance = 0.2126 * channel(background.r) + 0.7152 * channel(background.g) + 0.0722 * channel(background.b);
    const lighter = Math.max(foregroundLuminance, backgroundLuminance);
    const darker = Math.min(foregroundLuminance, backgroundLuminance);
    return (lighter + 0.05) / (darker + 0.05);
  }

  function label(element) {
    const tag = element.tagName.toLowerCase();
    const accessible = element.getAttribute('aria-label') || (element.textContent || '').trim().replace(/\s+/g, ' ');
    const shortened = accessible.slice(0, 80);
    return shortened ? '<' + tag + '> "' + shortened + '"' : '<' + tag + '>';
  }

  function isLargeText(fontSize, fontWeight) {
    const weight = fontWeight.toLowerCase() === 'bold' ? 700 : Number.parseInt(fontWeight, 10);
    return fontSize >= 24 || (fontSize >= 18.6666666667 && weight >= 700);
  }

  function effectiveBackground(element) {
    const layers = [];
    for (let current = element; current; current = current.parentElement) {
      const style = getComputedStyle(current);
      if (style.backgroundImage && style.backgroundImage !== 'none') return null;
      const color = parseColor(style.backgroundColor);
      if (color && color.a > 0) layers.push(color);
    }
    let background = { r: 255, g: 255, b: 255, a: 1 };
    for (let index = layers.length - 1; index >= 0; index -= 1) {
      background = composite(layers[index], background);
    }
    return background;
  }

  function hasVisibleDirectText(element) {
    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse') return false;
    for (let current = element; current; current = current.parentElement) {
      const currentStyle = getComputedStyle(current);
      const opacity = Number.parseFloat(currentStyle.opacity);
      if (Number.isFinite(opacity) && opacity < 1) return false;
      if (currentStyle.filter !== 'none' || currentStyle.mixBlendMode !== 'normal') return false;
    }
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return false;
    for (const child of element.childNodes) {
      if (child.nodeType === Node.TEXT_NODE && /\S/.test(child.nodeValue || '')) return true;
    }
    return false;
  }

  function measureContrast(element) {
    if (!hasVisibleDirectText(element)) return null;
    const style = getComputedStyle(element);
    const foreground = parseColor(style.color);
    const background = effectiveBackground(element);
    const fontSize = Number.parseFloat(style.fontSize);
    if (foreground === null || background === null || foreground.a <= 0 || !Number.isFinite(fontSize)) return null;
    const visibleForeground = composite(foreground, background);
    const minimum = isLargeText(fontSize, style.fontWeight) ? 3 : 4.5;
    const measuredRatio = ratio(visibleForeground, background);
    return measuredRatio < minimum ? { label: label(element), ratio: measuredRatio, minimum, fontSizePx: fontSize } : null;
  }

  function textNeighbor(parent, index, step) {
    for (let current = index + step; current >= 0 && current < parent.childNodes.length; current += step) {
      const node = parent.childNodes[current];
      if (/\S/.test(node.textContent || '')) return true;
    }
    return false;
  }

  function isInlineTextTarget(element, style) {
    if (style.display !== 'inline' || element.parentElement === null) return false;
    const siblings = [...element.parentElement.childNodes];
    const index = siblings.indexOf(element);
    return textNeighbor(element.parentElement, index, -1) || textNeighbor(element.parentElement, index, 1);
  }

  function isPointerTarget(element) {
    const tag = element.tagName.toLowerCase();
    if (tag === 'a' || tag === 'area') return element.hasAttribute('href');
    const role = (element.getAttribute('role') || '').toLowerCase();
    return ['button', 'link', 'checkbox', 'radio', 'switch', 'tab', 'option', 'menuitem', 'combobox'].includes(role);
  }

  function measurePointerTarget(element) {
    if (!isPointerTarget(element) || element.hasAttribute('disabled') || element.getAttribute('aria-disabled') === 'true') return null;
    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden' || style.pointerEvents === 'none' || isInlineTextTarget(element, style)) return null;
    const rect = element.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0 || (rect.width >= 24 && rect.height >= 24)) return null;
    return { label: label(element), width: rect.width, height: rect.height };
  }

  function measureOverflow(element) {
    const style = getComputedStyle(element);
    if (style.display === 'none' || style.visibility === 'hidden' || element.clientWidth <= 0) return null;
    return element.scrollWidth > element.clientWidth
      ? { label: label(element), scrollWidth: element.scrollWidth, clientWidth: element.clientWidth }
      : null;
  }

  function run() {
    const elements = [...document.querySelectorAll('*')];
    const findings = [];
    const contrastSamples = [];
    let contrastCount = 0;
    for (const element of elements) {
      const sample = measureContrast(element);
      if (sample !== null) {
        contrastCount += 1;
        if (contrastSamples.length < SAMPLE_LIMIT) contrastSamples.push(sample);
      }
    }
    if (contrastCount > 0) findings.push({ kind: 'contrast', count: contrastCount, samples: contrastSamples });

    const pointerSamples = [];
    let pointerCount = 0;
    for (const element of elements) {
      const sample = measurePointerTarget(element);
      if (sample !== null) {
        pointerCount += 1;
        if (pointerSamples.length < SAMPLE_LIMIT) pointerSamples.push(sample);
      }
    }
    if (pointerCount > 0) findings.push({ kind: 'pointer-target', count: pointerCount, samples: pointerSamples });

    const overflowSamples = [];
    let overflowCount = 0;
    for (const element of elements) {
      if (element === document.documentElement || element === document.body) continue;
      const sample = measureOverflow(element);
      if (sample !== null) {
        overflowCount += 1;
        if (overflowSamples.length < SAMPLE_LIMIT) overflowSamples.push(sample);
      }
    }
    const documentWidth = Math.max(document.documentElement ? document.documentElement.scrollWidth : 0, document.body ? document.body.scrollWidth : 0);
    const viewportWidth = window.innerWidth || (document.documentElement ? document.documentElement.clientWidth : 0);
    if (viewportWidth > 0 && documentWidth > viewportWidth) {
      overflowCount += 1;
      if (overflowSamples.length < SAMPLE_LIMIT) overflowSamples.push({ label: 'document', scrollWidth: documentWidth, clientWidth: viewportWidth });
    }
    if (overflowCount > 0) findings.push({ kind: 'overflow', count: overflowCount, samples: overflowSamples });
    return findings;
  }

  function report() {
    try {
      window.parent.postMessage({ kind: KIND, source: SOURCE, version: 1, findings: run() }, '*');
    } catch {
      // A missing result is safer than turning an evaluator failure into a finding.
    }
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', report, { once: true });
  } else {
    report();
  }
})();`;

function totalFindingCount(result: ArtifactRenderCriticResult): number {
  return result.findings.reduce((total, finding) => total + finding.count, 0);
}

function numberText(value: number): string {
  return Number.isInteger(value)
    ? String(value)
    : value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
}

function findingText(finding: ArtifactRenderFinding): string {
  if (finding.kind === "contrast") {
    const noun = finding.count === 1 ? "text element renders" : "text elements render";
    const samples = finding.samples
      .map(
        (sample) =>
          `Measured ${sample.label} at ${numberText(sample.ratio)}:1 (floor ${numberText(sample.minimum)}:1; ${numberText(sample.fontSizePx)} CSS px text).`,
      )
      .join(" ");
    return `Contrast: ${finding.count} ${noun} below its AA floor. ${samples}`.trim();
  }
  if (finding.kind === "pointer-target") {
    const noun = finding.count === 1 ? "target renders" : "targets render";
    const samples = finding.samples
      .map(
        (sample) =>
          `Measured ${sample.label} at ${numberText(sample.width)} × ${numberText(sample.height)} CSS px (minimum 24 × 24 CSS px).`,
      )
      .join(" ");
    return `Pointer targets: ${finding.count} ${noun} below the AA minimum. ${samples}`.trim();
  }
  const noun = finding.count === 1 ? "item measures" : "items measure";
  const samples = finding.samples
    .map(
      (sample) =>
        `Measured ${sample.label} at ${numberText(sample.scrollWidth)} CSS px wide for a ${numberText(sample.clientWidth)} CSS px client width.`,
    )
    .join(" ");
  return `Content overflow: ${finding.count} ${noun} wider than its available width. ${samples}`.trim();
}

function RenderCriticCard({ result }: { result: ArtifactRenderCriticResult }) {
  const total = totalFindingCount(result);
  return createElement(
    "div",
    { className: "design-canvas-artifact-render-warning", role: "status" },
    createElement(
      "div",
      null,
      `Render checks found ${total} ${total === 1 ? "finding" : "findings"}.`,
    ),
    createElement(
      "details",
      null,
      createElement("summary", null, "Inspect measurements"),
      createElement(
        "ul",
        null,
        result.findings.map((finding) =>
          createElement("li", { key: finding.kind }, findingText(finding)),
        ),
      ),
    ),
  );
}

export function ArtifactRenderCritic({ html }: { html: string }) {
  const [measurement, setMeasurement] = useState<{
    html: string;
    result: ArtifactRenderCriticResult;
  } | null>(null);

  useEffect(() => {
    let disposed = false;
    let settled = false;
    let timer: number | null = null;
    const frame = document.createElement("iframe");
    frame.className = "design-artifact-measurement-frame";
    frame.title = "";
    frame.setAttribute("aria-hidden", "true");
    frame.setAttribute("sandbox", ARTIFACT_RENDER_CRITIC_SANDBOX);
    frame.tabIndex = -1;

    const cleanup = () => {
      if (timer !== null) {
        window.clearTimeout(timer);
        timer = null;
      }
      window.removeEventListener("message", handleMessage);
      frame.remove();
    };
    const finish = (result: ArtifactRenderCriticResult) => {
      if (disposed || settled) return;
      settled = true;
      cleanup();
      setMeasurement({ html, result });
    };
    const handleMessage = (event: MessageEvent<unknown>) => {
      const result = readArtifactRenderCriticMessage(event, frame.contentWindow);
      if (result === null) return;
      finish(result);
    };

    window.addEventListener("message", handleMessage);
    timer = window.setTimeout(() => {
      if (disposed || settled) return;
      settled = true;
      cleanup();
    }, ARTIFACT_RENDER_CRITIC_TIMEOUT_MS);
    frame.srcdoc = buildArtifactMeasurementSrcDoc(html);
    document.body.append(frame);

    return () => {
      disposed = true;
      settled = true;
      cleanup();
    };
  }, [html]);

  const result = measurement?.html === html ? measurement.result : null;
  if (result === null || result.findings.length === 0) return null;
  return createElement(RenderCriticCard, { result });
}
