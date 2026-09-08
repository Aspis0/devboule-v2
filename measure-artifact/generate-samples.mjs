/**
 * Build scaled artifact HTML for the Design-surface size measurement.
 *
 * Families:
 *   realistic   — a self-contained invoices page: nested layout, many small
 *                 elements, a :root token stylesheet, inline SVG, focus rules.
 *                 Size grows by repeating a card unit, not by padding comments.
 *   adversarial — a minimal document whose remaining bytes are one HTML comment.
 *
 * Targets are exact UTF-8 byte counts (ASCII-only markup, so bytes === length).
 */
import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const samplesDir = path.join(here, "samples");

export const SIZES_KIB = [8, 16, 32, 64, 128, 256, 384, 512, 768, 1024, 1536, 2048, 3072];

const PAD = "[[PAD]]";

const STYLESHEET = `:root {
  --surface: #f4f1ea;
  --surface-raised: #fffdf8;
  --surface-sunken: #ece7dc;
  --ink: #1c1915;
  --ink-soft: #5c564c;
  --ink-faint: #8a8276;
  --accent: #1f6f5b;
  --accent-strong: #155445;
  --accent-soft: #dcebe5;
  --danger: #9b2c2c;
  --danger-surface: #f8e4e4;
  --warning: #8a5a12;
  --warning-surface: #f6ead2;
  --success: #215c3a;
  --success-surface: #dcefe3;
  --border: #e0d8c8;
  --focus: #1f6f5b;
  --shadow: 0 1px 2px rgba(28, 25, 21, 0.08);
  --space-1: 4px;
  --space-2: 8px;
  --space-3: 12px;
  --space-4: 16px;
  --space-5: 24px;
  --space-6: 32px;
  --radius: 10px;
  --font: system-ui, "Segoe UI", sans-serif;
  --text-xs: 0.75rem;
  --text-sm: 0.875rem;
  --text-md: 1rem;
  --text-lg: 1.25rem;
  --text-xl: 1.6rem;
  --leading: 1.45;
  --sidebar: 220px;
}
*, *::before, *::after { box-sizing: border-box; }
html, body { margin: 0; min-height: 100%; }
body {
  font-family: var(--font);
  font-size: var(--text-md);
  line-height: var(--leading);
  color: var(--ink);
  background: var(--surface);
}
.shell { display: grid; grid-template-columns: var(--sidebar) 1fr; min-height: 100vh; }
.rail {
  display: flex;
  flex-direction: column;
  gap: var(--space-2);
  padding: var(--space-5);
  background: var(--surface-sunken);
  border-right: 1px solid var(--border);
}
.rail a {
  display: flex;
  align-items: center;
  gap: var(--space-2);
  min-height: 24px;
  color: var(--ink);
  text-decoration: none;
  padding: var(--space-2) var(--space-3);
  border-radius: var(--radius);
}
.rail a[aria-current="page"] { background: var(--accent-soft); color: var(--accent-strong); }
.main { display: flex; flex-direction: column; gap: var(--space-5); padding: var(--space-6); }
.page-head { display: flex; justify-content: space-between; align-items: baseline; gap: var(--space-4); }
.page-head h1 { margin: 0; font-size: var(--text-xl); font-weight: 600; }
.lede { margin: 0; color: var(--ink-soft); max-width: 65ch; }
.toolbar { display: flex; flex-wrap: wrap; gap: var(--space-3); align-items: end; }
label { display: grid; gap: var(--space-1); font-size: var(--text-sm); color: var(--ink-soft); }
input, select, button, a.button {
  font: inherit;
  color: var(--ink);
  background: var(--surface-raised);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  min-height: 24px;
  padding: var(--space-2) var(--space-3);
}
button, a.button {
  background: var(--accent);
  color: var(--surface-raised);
  border-color: var(--accent-strong);
  cursor: pointer;
}
button.ghost { background: var(--surface-raised); color: var(--ink); border-color: var(--border); }
.grid { display: grid; gap: var(--space-4); }
.card {
  display: grid;
  gap: var(--space-3);
  padding: var(--space-4);
  background: var(--surface-raised);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
}
.card-head { display: flex; align-items: center; gap: var(--space-2); }
.card-head h2 { margin: 0; font-size: var(--text-lg); font-weight: 600; flex: 1; }
.badge {
  display: inline-flex;
  align-items: center;
  min-height: 24px;
  padding: 0 var(--space-2);
  border-radius: 999px;
  background: var(--accent-soft);
  color: var(--accent-strong);
  font-size: var(--text-xs);
}
.badge.warn { background: var(--warning-surface); color: var(--warning); }
.badge.danger { background: var(--danger-surface); color: var(--danger); }
.meta { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: var(--space-3); margin: 0; }
.meta div { display: grid; gap: var(--space-1); }
.meta dt { color: var(--ink-faint); font-size: var(--text-xs); }
.meta dd { margin: 0; }
.lines { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--space-2); }
.lines li { display: flex; justify-content: space-between; gap: var(--space-4); border-top: 1px solid var(--border); padding-top: var(--space-2); }
.actions { display: flex; flex-wrap: wrap; gap: var(--space-2); align-items: end; }
.icon { width: 16px; height: 16px; flex: none; color: var(--accent); }
.end-note { color: var(--ink-faint); font-size: var(--text-sm); }
:focus-visible { outline: 2px solid var(--focus); outline-offset: 2px; }
button:focus-visible { outline-color: var(--focus); }
a:focus-visible { outline-color: var(--accent); }
input:focus-visible { outline-color: var(--accent-strong); }
select:focus-visible { outline-color: var(--accent); }
.badge:focus-visible { outline-color: var(--warning); }
.rail a:focus-visible { outline-color: var(--accent-strong); }
.ghost:focus-visible { outline-color: var(--ink); }
.button:focus-visible { outline-color: var(--success); }
.card a:focus-visible { outline-color: var(--danger); }
`;

const ICON = `<svg class="icon" viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><path fill="currentColor" d="M12 2a10 10 0 1 0 10 10A10 10 0 0 0 12 2zm1 15h-2v-2h2zm0-4h-2V7h2z"/></svg>`;

function card(index) {
  const id = String(index).padStart(5, "0");
  const amount = (1200 + (index % 97) * 15).toFixed(2);
  const status = index % 7 === 0 ? "Overdue" : index % 3 === 0 ? "Review" : "Open";
  const badgeClass = status === "Overdue" ? "badge danger" : status === "Review" ? "badge warn" : "badge";
  return `<article class="card" id="inv-${id}">
<header class="card-head">${ICON}<h2 style="color: var(--ink)">Invoice ${id}</h2><span class="${badgeClass}">${status}</span></header>
<p class="lede" style="color: var(--ink-soft)">Due 12 May · Northwind ${id} · net 30 on the original purchase order for warehouse restock and packing.</p>
<dl class="meta">
<div><dt>Owner</dt><dd>Lina Ortega</dd></div>
<div><dt>Amount</dt><dd>$${amount}</dd></div>
<div><dt>Status</dt><dd>Awaiting ${status === "Open" ? "payment" : "action"}</dd></div>
</dl>
<ul class="lines">
<li><span style="color: var(--ink)">Paper stock, A4</span><strong style="color: var(--ink)">64.00</strong></li>
<li><span style="color: var(--ink-soft)">Freight, zone 2</span><strong>18.50</strong></li>
<li><span style="color: var(--ink-soft)">Rush handling</span><strong>12.00</strong></li>
</ul>
<form class="actions">
<label for="note-${id}">Note<input id="note-${id}" name="note" type="text" value="Remind on Friday"></label>
<button type="button">Remind</button>
<a class="button ghost" href="#inv-${id}">Open record</a>
</form>
</article>`;
}

function realisticShell(cardsHtml, padText) {
  return `<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>Northwind — Invoices</title>
<style>
${STYLESHEET}
</style>
</head>
<body>
<div class="shell">
<aside class="rail">
<a href="#home">${ICON}<span>Home</span></a>
<a href="#invoices" aria-current="page">${ICON}<span>Invoices</span></a>
<a href="#vendors">${ICON}<span>Vendors</span></a>
<a href="#settings">${ICON}<span>Settings</span></a>
</aside>
<div class="main">
<header class="page-head">
<div>
<h1>Open invoices</h1>
<p class="lede">A working list, not a dashboard. Each row is a record a person can act on today.</p>
</div>
<button type="button">New invoice</button>
</header>
<section class="toolbar">
<label>Search<input type="search" name="q" value="" placeholder="Vendor or number"></label>
<label>Status<select name="status"><option>Any</option><option>Open</option><option>Review</option></select></label>
<button type="button" class="ghost">Export</button>
</section>
<section class="grid" aria-label="Invoice list">
${cardsHtml}
</section>
<p class="end-note">${padText}</p>
</div>
</div>
</body>
</html>
`;
}

function bytes(text) {
  return Buffer.byteLength(text, "utf8");
}

function countStartTags(html) {
  return (html.match(/<[A-Za-z][A-Za-z0-9:-]*/g) || []).length;
}

function countMatches(html, pattern) {
  return (html.match(pattern) || []).length;
}

function buildRealistic(targetBytes) {
  const empty = realisticShell("", PAD);
  const emptyBytes = bytes(empty);
  if (emptyBytes > targetBytes) {
    throw new Error(`realistic shell is ${emptyBytes} bytes, larger than ${targetBytes}`);
  }
  const oneCard = card(1);
  const cardBytes = bytes(oneCard) + 1;
  const maxCards = Math.max(0, Math.floor((targetBytes - emptyBytes) / cardBytes));
  let lo = 0;
  let hi = maxCards + 2;
  while (lo < hi) {
    const mid = Math.ceil((lo + hi) / 2);
    const candidate = realisticShell(Array.from({ length: mid }, (_, i) => card(i + 1)).join("\n"), PAD);
    if (bytes(candidate) <= targetBytes) lo = mid;
    else hi = mid - 1;
  }
  const cards = Array.from({ length: lo }, (_, i) => card(i + 1)).join("\n");
  const withPadMarker = realisticShell(cards, PAD);
  const current = bytes(withPadMarker);
  if (current > targetBytes) {
    throw new Error(`realistic overshot: ${current} > ${targetBytes} with ${lo} cards`);
  }
  const padLength = targetBytes - current + PAD.length;
  const padText = padLength <= PAD.length ? "x".repeat(padLength) : "x".repeat(padLength);
  const html = realisticShell(cards, padText);
  if (bytes(html) !== targetBytes) {
    throw new Error(`realistic size miss: ${bytes(html)} !== ${targetBytes}`);
  }
  return { html, cards: lo };
}

function buildAdversarial(targetBytes) {
  const prefix = `<!DOCTYPE html><html lang="en"><head><meta charset="utf-8"><title>x</title></head><body><!--`;
  const suffix = `--></body></html>`;
  const inner = targetBytes - bytes(prefix + suffix);
  if (inner < 0) throw new Error(`adversarial shell larger than ${targetBytes}`);
  const html = prefix + "A".repeat(inner) + suffix;
  if (bytes(html) !== targetBytes) {
    throw new Error(`adversarial size miss: ${bytes(html)} !== ${targetBytes}`);
  }
  return { html };
}

function describe(family, kib, html, extra) {
  return {
    family,
    kib,
    targetBytes: kib * 1024,
    actualBytes: bytes(html),
    startTags: countStartTags(html),
    customPropertyDefs: countMatches(html, /--[A-Za-z_][A-Za-z0-9_-]*\s*:/g),
    varReferences: countMatches(html, /\bvar\s*\(/g),
    svgCount: countMatches(html, /<svg\b/g),
    buttonCount: countMatches(html, /<button\b/g),
    inputCount: countMatches(html, /<(input|select)\b/g),
    anchorCount: countMatches(html, /<a\b/g),
    focusRules: countMatches(html, /:focus-visible/g),
    ...extra,
    file: `measure-artifact/samples/${family}-${String(kib).padStart(4, "0")}kib.html`,
  };
}

export function generateSamples() {
  mkdirSync(samplesDir, { recursive: true });
  const manifest = [];
  for (const kib of SIZES_KIB) {
    const targetBytes = kib * 1024;
    const realistic = buildRealistic(targetBytes);
    const realisticPath = path.join(samplesDir, `realistic-${String(kib).padStart(4, "0")}kib.html`);
    writeFileSync(realisticPath, realistic.html);
    manifest.push(describe("realistic", kib, realistic.html, { cards: realistic.cards }));

    const adversarial = buildAdversarial(targetBytes);
    const adversarialPath = path.join(samplesDir, `adversarial-${String(kib).padStart(4, "0")}kib.html`);
    writeFileSync(adversarialPath, adversarial.html);
    manifest.push(describe("adversarial", kib, adversarial.html, { cards: 0 }));
  }
  const manifestPath = path.join(here, "manifest.json");
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
  return manifest;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const manifest = generateSamples();
  for (const row of manifest) {
    console.log(
      `${row.family.padEnd(12)} ${String(row.kib).padStart(4)} KiB  bytes=${row.actualBytes}  tags=${row.startTags}  cards=${row.cards}  var()=${row.varReferences}`,
    );
  }
}
