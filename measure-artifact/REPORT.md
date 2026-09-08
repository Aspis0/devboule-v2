# Artifact size vs Design-surface cost

**Question:** is `MAX_ARTIFACT_BYTES = 256 KiB` above, below, or near the size at which the Design surface actually degrades — and what degrades first?

**Answer:** on this machine, 256 KiB of *realistic* HTML is **safe and not generous**. Nothing fails. The render critic is already using **~74% of its 1,500 ms timeout** (median 1,117 ms, max 1,127 ms, 0/5 timeouts). The first hard failure is the critic silently producing nothing, and it is **structure**, not bytes, that trips it.

---

## The single most important line

| Family | Size at which the render critic exceeds its own 1,500 ms timeout |
| --- | --- |
| **realistic** (nested layout, tokens, SVG, many small elements) | **768 KiB: 5/5 isolated trials miss** (median 1,936 ms). Combined with the preview iframe, **512 KiB already misses in 2/5** (median 1,477 ms, two trials at 1,531 ms and 1,544 ms). Isolated 512 KiB still fits (median 1,343 ms, max 1,392 ms, 0/5). Linear interpolation of isolated medians puts the 1,500 ms crossing at **~580 KiB**. |
| **adversarial** (minimal document + one HTML comment filling the rest) | **Does not exceed through 3,072 KiB** (median 97 ms at 3 MiB). |

256 KiB realistic: critic median **1,117 ms** [1,001–1,127], **0/5** over 1,500 ms.

---

## Machine and browser

| | |
| --- | --- |
| OS | Windows 11 Home (`Windows_NT 10.0.26200`) |
| CPU | Intel Core Ultra 9 185H, 16 cores / 22 threads |
| RAM | 31.6 GB |
| Browser | **Edge 152.0.4191.66** headless (`HeadlessChrome/152.0.0.0`, V8 15.2.23.10, revision `@cc2931e6363af1d70882ad63ee33b0e8cd524de0`) |
| Why Edge | Closest installed Chromium to the app's WebView2. The critic's own comment was also taken on Chromium 152. |
| Date | 2026-09-08 |
| Raw data | `measure-artifact/results.json`, `measure-artifact/summary.json`, `measure-artifact/machine.json` |

This is a fast laptop. Numbers will move up on slower hardware.

---

## What was measured (product path, not a reimplementation)

Constants read from the **real modules** at run time:

- `ARTIFACT_RENDER_CRITIC_TIMEOUT_MS = 1_500` (`src/features/design/artifactRenderCritic.ts:35`)
- `ARTIFACT_RENDER_CRITIC_SANDBOX = "allow-scripts"`
- `findUndefinedCustomProperties` from `src/features/design/artifactTokenLint.ts:115` (called in `DesignSurface.tsx:1771`)
- Critic srcdoc from `buildArtifactMeasurementSrcDoc` (strip scripts, inject CSP + measurement script, postMessage)
- Preview srcdoc = CSP meta + HTML, copied from `DesignSurface.tsx:491–524`, iframe `sandbox=""` (empty), **700×500** CSS px (the canvas artifact node)

The product starts the 1,500 ms timer, then calls `buildArtifactMeasurementSrcDoc` and sets `srcdoc`. Wall-clock here includes that build.

Per size × family: 1 discarded warmup, then **5 trials** each of isolated token-lint, isolated preview, isolated critic, and combined (tokens → preview iframe → rAF → critic iframe, overlapping). Report is **median [min–max]**.

Sizes: 8, 16, 32, 64, 128, 256, 384, 512, 768, 1,024, 1,536, 2,048, 3,072 KiB. Exact UTF-8 byte counts.

---

## Families

**(a) realistic.** Self-contained invoices page following the craft doctrine: `:root` tokens named by role, spacing scale, nested shell (sidebar + main + card grid), inline 24×24 SVG icons, `:focus-visible` rules, buttons/inputs/links with 24 px min-height. Size grows by **repeating a card**, not by padding comments. At 256 KiB: **208 cards, 6,697 start-tags, 1,325 `var()`, 10 `:focus-visible` rules**.

**(b) adversarial.** `<!DOCTYPE html>…<body><!-- AAA… --></body></html>` padded to the same byte count. Always **5 start-tags**. This is the trap the byte cap cannot see.

---

## Table: isolated metrics, realistic family

Times in milliseconds. Median [min–max]. `ex` = isolated critic trials with wall-clock > 1,500 ms (the product timeout). `cap` = trials that hit the 30 s measurement cap (none did).

| KiB | tags | cards | token lint | preview load+2rAF | critic wall (build+parse+run) | critic build only | ex/5 | critic findings |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 8 | 73 | 1 | 0.4 [0.3–0.7] | 73 [66–165] | 66 [60–82] | 0.7 [0.5–1.0] | 0 | 0 |
| 16 | 297 | 8 | 0.6 [0.5–1.0] | 79 [71–87] | 89 [84–110] | 2.2 [1.4–2.6] | 0 | 0 |
| 32 | 713 | 21 | 1.3 [1.0–1.5] | 88 [81–97] | 229 [160–**2,307**] | 5.3 [3.4–5.6] | **1** | 0 |
| 64 | 1,577 | 48 | 3.3 [2.3–4.3] | 85 [81–102] | 255 [227–258] | 8.8 [7.1–10.9] | 0 | 0 |
| 128 | 3,273 | 101 | 5.4 [3.9–9.7] | 117 [112–125] | 449 [325–572] | 15 [12–17] | 0 | 306 |
| **256** | **6,697** | **208** | **9.5 [7.5–12.2]** | **199 [197–201]** | **1,117 [1,001–1,127]** | **36 [23–38]** | **0** | 627 |
| 384 | 10,089 | 314 | 19 [12–19] | 233 [232–234] | 1,136 [1,047–1,347] | 33 [33–49] | 0 | 945 |
| 512 | 13,513 | 421 | 25 [23–26] | 416 [412–418] | 1,343 [1,302–1,392] | 45 [43–55] | 0 | 1,266 |
| **768** | **20,361** | **635** | 22 [22–27] | 615 [551–655] | **1,936 [1,893–1,984]** | 69 [62–86] | **5** | 1,908 |
| 1,024 | 27,177 | 848 | 30 [29–32] | 799 [782–822] | 2,515 [2,510–2,687] | 99 [92–125] | 5 | 2,547 |
| 1,536 | 40,841 | 1,275 | 45 [44–49] | 1,249 [1,150–1,266] | 3,764 [3,752–4,029] | 139 [127–208] | 5 | 3,828 |
| 2,048 | 54,505 | 1,702 | 59 [57–65] | 1,550 [1,534–1,663] | 5,061 [4,910–6,796] | 186 [177–201] | 5 | 5,109 |
| 3,072 | 81,833 | 2,556 | 89 [85–108] | 2,214 [2,148–2,458] | 7,572 [7,510–10,361] | 293 [275–305] | 5 | 7,671 |

The 32 KiB critic max of 2,307 ms is **one trial** (the other four were 160–248 ms). Combined at 32 KiB did not timeout. Treat it as a tail spike, not a size threshold. It is still a real observation: a single critic run can blow 1,500 ms far below the size where the median does.

From 768 KiB up, critic wall-clock is ~0.093 ms per start-tag on this machine.

---

## Table: isolated metrics, adversarial family

Same byte counts, 5 tags every time.

| KiB | tags | token lint | preview load+2rAF | critic wall | ex/5 |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 8 | 5 | 0.2 [0.1–0.7] | 62 [48–65] | 38 [30–49] | 0 |
| 16 | 5 | 0.3 [0.2–0.4] | 48 [42–50] | 47 [25–117] | 0 |
| 32 | 5 | 0.6 [0.6–0.7] | 63 [38–98] | 42 [28–50] | 0 |
| 64 | 5 | 0.9 [0.9–1.3] | 49 [48–54] | 38 [31–46] | 0 |
| 128 | 5 | 2.1 [1.7–3.0] | 48 [47–49] | 40 [36–43] | 0 |
| **256** | **5** | **3.1 [3.0–3.3]** | **49 [47–64]** | **36 [31–46]** | **0** |
| 384 | 5 | 4.8 [4.7–4.9] | 49 [42–115] | 37 [31–44] | 0 |
| 512 | 5 | 9.2 [7.0–11] | 49 [47–131] | 61 [58–82] | 0 |
| 768 | 5 | 10 [9.3–11] | 49 [46–49] | 36 [34–112] | 0 |
| 1,024 | 5 | 12 [12–13] | 66 [65–68] | 51 [45–90] | 0 |
| 1,536 | 5 | 20 [17–24] | 65 [61–99] | 64 [60–65] | 0 |
| 2,048 | 5 | 24 [23–25] | 66 [65–90] | 71 [67–76] | 0 |
| 3,072 | 5 | 36 [35–40] | 105 [97–117] | 97 [89–102] | 0 |

At 256 KiB the two families differ by **~30×** on the critic (1,117 ms vs 36 ms). A byte cap cannot tell them apart.

---

## Combined path (preview + critic overlapping)

Product order: `findUndefinedCustomProperties` during render, preview `<iframe sandbox="">` on commit, critic iframe in `useEffect` after paint. Combined trials ran **after** the isolated critic trials, so they are hotter. Isolated is the more conservative stand-in for a first paint.

| KiB | family | combined critic median [min–max] | combined exceeds 1,500 | combined total |
| ---: | --- | --- | ---: | --- |
| 256 | realistic | 799 [775–802] | 0/5 | 812 [789–819] |
| 384 | realistic | 1,170 [1,138–1,200] | 0/5 | 1,187 [1,157–1,220] |
| **512** | **realistic** | **1,477 [1,430–1,544]** | **2/5** | **1,495 [1,449–1,563]** |
| 768 | realistic | 2,168 [2,132–2,222] | 5/5 | 2,200 [2,158–2,248] |
| 256 | adversarial | 41 [32–47] | 0/5 | — |
| 3,072 | adversarial | 119 [117–124] | 0/5 | — |

---

## What degrades first

Order, for the **realistic** family, on this machine:

1. **Render critic timeout (silent no-result).** This is the first *already-existing* hard failure. Isolated: always past 1,500 ms at **768 KiB**. Combined: starts missing at **512 KiB** (2/5). At 256 KiB it still returns, with ~380 ms of budget left on the median (1,500 − 1,117).
2. **Preview parse + layout hitch.** No timeout in the product; the user just waits. 199 ms at 256 KiB (already more than one long-task budget of 50 ms, but the parent `PerformanceObserver` did **not** see it — unique-origin sandboxed iframes do not show up as parent long tasks). Crosses ~1 s around 1,536 KiB (1,249 ms) and ~1,500 ms around 2,048 KiB (1,550 ms). Degrades *earlier* as a hitch, but does not fail closed.
3. **Parent-visible long tasks (>50 ms).** First appear around 384–512 KiB on the critic/combined path (52–72 ms), then consistently from 768 KiB (median-of-max 73 ms → 309 ms at 3,072). These under-count iframe work.
4. **`findUndefinedCustomProperties`.** Last. 9.5 ms at 256 KiB, 89 ms at 3,072 KiB. It always `split("")`s the whole string (`artifactTokenLint.ts`), so cost tracks bytes more than tree shape — which is why adversarial lint is 3.1 ms at 256 KiB and 36 ms at 3,072 KiB. Never the limiter.

Adversarial bytes do not degrade the critic or the preview in this range. That is the point of family (b).

The existing comment in `artifactRenderCritic.ts` measured **`run()` only** at 242.9 ms on a 261,117-byte synthetic with 4,000 controls. Wall-clock here is build + parse + layout + `run()` + `postMessage`. At 256 KiB realistic that total is 1,117 ms, of which ~36 ms is `buildArtifactMeasurementSrcDoc`. The rest is the iframe.

---

## Is 256 KiB safe, generous, or already too large?

**Safe on this machine, not generous, not already too large.**

- Isolated critic at 256 KiB: median 1,117 ms, max 1,127 ms, **0/5** timeouts. Margin to 1,500 ms: **~380 ms, about 1.34×**.
- Combined (hot) at 256 KiB: 799 ms, about 1.9×. Do not lean on that; it is after five isolated critic runs.
- Preview at 256 KiB: 199 ms. Perceptible hitch, not a failure.
- Token lint at 256 KiB: 9.5 ms. Noise.
- 512 KiB is where the product-like path **starts** dropping critic results (2/5). 768 KiB is where isolated critic **always** drops them.
- 3,072 KiB of comments is still 97 ms. The cap is not measuring the thing that costs.

**Number I would defend:** keep **256 KiB** as a byte ceiling if the goal is “do not feed the critic a document that already spends most of 1,500 ms on a fast CPU.” I would not raise it. 128 KiB is the generous point on this box (critic median 449 ms, ~3.3× margin). 512 KiB is already on the wrong side of the timeout under combined load.

A byte cap remains a weak proxy. 256 KiB realistic ≈ 1,117 ms critic; 256 KiB comments ≈ 36 ms. Any cap in bytes will both reject cheap documents and admit expensive ones, depending on tree shape.

---

## What this did **not** measure

- **The running Tauri WebView2.** This was Edge 152 headless, same Chromium generation, not the app process, not with Pixi/layers/React chrome on the same renderer.
- **Low-end CPUs, thermal throttle, battery, 4× CPU slowdown.** Ultra 9 185H is an upper bound on “fast.” 1.34× margin here can vanish there.
- **Cold first generation** with no warmup. One combined warmup was discarded per size. A 32 KiB isolated trial still hit 2,307 ms; that class of spike at 256 KiB would miss the timeout.
- **Process RSS / GPU / compositor memory.** Only `performance.memory.usedJSHeapSize`, which is GC-noisy (a few MB at 256 KiB, ~48 MB JS heap at 3,072 KiB tokens/preview). Not peak process memory.
- **Parent long tasks caused by iframe layout.** `PerformanceObserver({type:"longtask"})` on the parent did not record the 199–2,214 ms preview layouts (unique-origin sandbox). Critic wall-clock is the number to trust.
- **A 4,000-control synthetic matching the file comment.** Realistic cards are a different mix (more nesting, SVG, tokens, fewer raw `<button>`s per byte).
- **Stylesheet-heavy artifacts.** CSS was a fixed ~3–4 KiB; size scaled the DOM.
- **jsdom / happy-dom.** Intentionally not used.
- **Changing sandbox, CSP, or srcdoc construction.** Measured as they are.

---

## How to reproduce

From the repo root (does not touch `src/`):

```
node measure-artifact/run.mjs
```

Writes `measure-artifact/results.json`. Harness imports the real TypeScript modules through Vite on `127.0.0.1:4177`. Samples are in `measure-artifact/samples/`.
