import {
  ARTIFACT_RENDER_CRITIC_SANDBOX,
  ARTIFACT_RENDER_CRITIC_TIMEOUT_MS,
  buildArtifactMeasurementSrcDoc,
  readArtifactRenderCriticMessage,
} from "../src/features/design/artifactRenderCritic";
import { findUndefinedCustomProperties } from "../src/features/design/artifactTokenLint";

// Snapshot of DesignSurface.tsx:491-524. Copied, not imported: artifactSrcDoc is not exported.
const PREVIEW_CSP =
  "default-src 'none'; img-src data:; style-src 'unsafe-inline'; script-src 'none'; font-src 'none'; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; object-src 'none'; media-src 'none'; worker-src 'none'; manifest-src 'none'";
const PREVIEW_CSP_META = `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}" />`;

function artifactSrcDoc(html: string): string {
  return `${PREVIEW_CSP_META}\n${html}`;
}

const TRIALS = 5;
const CRITIC_CAP_MS = 30_000;
const PREVIEW_CAP_MS = 30_000;

type LongTask = { duration: number; startTime: number };
type MemorySnap = { used: number; total: number } | null;

type ManifestEntry = {
  family: string;
  kib: number;
  targetBytes: number;
  actualBytes: number;
  startTags: number;
  customPropertyDefs: number;
  varReferences: number;
  svgCount: number;
  buttonCount: number;
  inputCount: number;
  anchorCount: number;
  focusRules: number;
  cards: number;
  file: string;
};

function status(text: string): void {
  const node = document.getElementById("status");
  if (node) node.textContent = text;
  console.log(text);
}

function memory(): MemorySnap {
  const perf = performance as Performance & {
    memory?: { usedJSHeapSize: number; totalJSHeapSize: number };
  };
  if (!perf.memory) return null;
  return { used: perf.memory.usedJSHeapSize, total: perf.memory.totalJSHeapSize };
}

class LongTaskProbe {
  private entries: LongTask[] = [];
  private observer: PerformanceObserver | null = null;

  start(): void {
    this.entries = [];
    try {
      this.observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          this.entries.push({ duration: entry.duration, startTime: entry.startTime });
        }
      });
      this.observer.observe({ type: "longtask" });
    } catch {
      this.observer = null;
    }
  }

  stop(): LongTask[] {
    this.observer?.disconnect();
    this.observer = null;
    return this.entries.filter((entry) => entry.duration > 50);
  }
}

function stats(values: number[]): {
  n: number;
  min: number;
  max: number;
  median: number;
  mean: number;
  p90: number;
  spread: number;
} | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  const median =
    sorted.length % 2 === 1 ? sorted[mid]! : (sorted[mid - 1]! + sorted[mid]!) / 2;
  const mean = values.reduce((sum, value) => sum + value, 0) / values.length;
  const p90Index = Math.min(sorted.length - 1, Math.max(0, Math.ceil(sorted.length * 0.9) - 1));
  return {
    n: values.length,
    min: sorted[0]!,
    max: sorted[sorted.length - 1]!,
    median,
    mean,
    p90: sorted[p90Index]!,
    spread: sorted[sorted.length - 1]! - sorted[0]!,
  };
}

function nextFrame(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => resolve());
  });
}

function twoFrames(): Promise<void> {
  return new Promise((resolve) => {
    requestAnimationFrame(() => {
      requestAnimationFrame(() => resolve());
    });
  });
}

function maybeGc(): void {
  const gc = (window as unknown as { gc?: () => void }).gc;
  if (typeof gc === "function") gc();
}

function previewBox(): HTMLElement {
  const box = document.getElementById("preview-box");
  if (!box) throw new Error("preview-box missing");
  return box;
}

function makePreviewFrame(): HTMLIFrameElement {
  const frame = document.createElement("iframe");
  frame.setAttribute("sandbox", "");
  frame.className = "design-artifact-frame";
  frame.title = "Generated artifact";
  frame.style.pointerEvents = "none";
  return frame;
}

function makeCriticFrame(): HTMLIFrameElement {
  const frame = document.createElement("iframe");
  frame.className = "design-artifact-measurement-frame";
  frame.title = "";
  frame.setAttribute("aria-hidden", "true");
  frame.setAttribute("sandbox", ARTIFACT_RENDER_CRITIC_SANDBOX);
  frame.tabIndex = -1;
  return frame;
}

type PreviewMeasure = {
  loadMs: number;
  layoutMs: number;
  timedOut: boolean;
  longTasks: LongTask[];
  memBefore: MemorySnap;
  memAfter: MemorySnap;
};

type CriticMeasure = {
  totalMs: number;
  buildMs: number;
  timedOutAtCap: boolean;
  exceedsProductTimeout: boolean;
  findingCount: number | null;
  longTasks: LongTask[];
  memBefore: MemorySnap;
  memAfter: MemorySnap;
};

type TokenMeasure = {
  ms: number;
  missingCount: number;
  longTasks: LongTask[];
  memBefore: MemorySnap;
  memAfter: MemorySnap;
};

function waitForLoad(frame: HTMLIFrameElement, capMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    const timer = window.setTimeout(() => resolve(false), capMs);
    frame.addEventListener(
      "load",
      () => {
        window.clearTimeout(timer);
        resolve(true);
      },
      { once: true },
    );
  });
}

async function measurePreview(html: string): Promise<PreviewMeasure> {
  const probe = new LongTaskProbe();
  probe.start();
  const memBefore = memory();
  const box = previewBox();
  box.replaceChildren();
  const frame = makePreviewFrame();
  const srcDoc = artifactSrcDoc(html);
  const loaded = waitForLoad(frame, PREVIEW_CAP_MS);
  const t0 = performance.now();
  frame.srcdoc = srcDoc;
  box.append(frame);
  const ok = await loaded;
  const loadMs = performance.now() - t0;
  if (ok) await twoFrames();
  const layoutMs = performance.now() - t0;
  const longTasks = probe.stop();
  const memAfter = memory();
  frame.remove();
  box.replaceChildren();
  return { loadMs, layoutMs, timedOut: !ok, longTasks, memBefore, memAfter };
}

function measureCritic(html: string): Promise<CriticMeasure> {
  const probe = new LongTaskProbe();
  probe.start();
  const memBefore = memory();
  const frame = makeCriticFrame();
  const t0 = performance.now();
  let buildMs = 0;

  return new Promise((resolve) => {
    let settled = false;
    const finish = (partial: {
      timedOutAtCap: boolean;
      findingCount: number | null;
    }) => {
      if (settled) return;
      settled = true;
      window.clearTimeout(timer);
      window.removeEventListener("message", handleMessage);
      frame.remove();
      const totalMs = performance.now() - t0;
      resolve({
        totalMs,
        buildMs,
        timedOutAtCap: partial.timedOutAtCap,
        exceedsProductTimeout: totalMs > ARTIFACT_RENDER_CRITIC_TIMEOUT_MS,
        findingCount: partial.findingCount,
        longTasks: probe.stop(),
        memBefore,
        memAfter: memory(),
      });
    };

    const handleMessage = (event: MessageEvent<unknown>) => {
      const result = readArtifactRenderCriticMessage(event, frame.contentWindow);
      if (result === null) return;
      const findingCount = result.findings.reduce((sum, finding) => sum + finding.count, 0);
      finish({ timedOutAtCap: false, findingCount });
    };

    window.addEventListener("message", handleMessage);
    const timer = window.setTimeout(() => {
      finish({ timedOutAtCap: true, findingCount: null });
    }, CRITIC_CAP_MS);

    const built = buildArtifactMeasurementSrcDoc(html);
    buildMs = performance.now() - t0;
    frame.srcdoc = built;
    document.body.append(frame);
  });
}

function measureTokens(html: string): TokenMeasure {
  const probe = new LongTaskProbe();
  probe.start();
  const memBefore = memory();
  const t0 = performance.now();
  const missing = findUndefinedCustomProperties(html);
  const ms = performance.now() - t0;
  return {
    ms,
    missingCount: missing.length,
    longTasks: probe.stop(),
    memBefore,
    memAfter: memory(),
  };
}

type CombinedMeasure = {
  tokensMs: number;
  previewLoadMs: number;
  previewLayoutMs: number;
  previewTimedOut: boolean;
  criticTotalMs: number;
  criticBuildMs: number;
  criticTimedOutAtCap: boolean;
  criticExceedsProductTimeout: boolean;
  criticFindingCount: number | null;
  totalMs: number;
  longTasks: LongTask[];
  memBefore: MemorySnap;
  memAfter: MemorySnap;
};

async function measureCombined(html: string): Promise<CombinedMeasure> {
  const probe = new LongTaskProbe();
  probe.start();
  const memBefore = memory();
  const t0 = performance.now();
  const missing = findUndefinedCustomProperties(html);
  const tokensMs = performance.now() - t0;
  void missing;

  const box = previewBox();
  box.replaceChildren();
  const previewFrame = makePreviewFrame();
  const previewLoaded = waitForLoad(previewFrame, PREVIEW_CAP_MS);
  const previewT0 = performance.now();
  previewFrame.srcdoc = artifactSrcDoc(html);
  box.append(previewFrame);

  await nextFrame();

  const criticPromise = measureCritic(html);
  const previewOk = await previewLoaded;
  const previewLoadMs = performance.now() - previewT0;
  if (previewOk) await twoFrames();
  const previewLayoutMs = performance.now() - previewT0;
  const critic = await criticPromise;
  previewFrame.remove();
  box.replaceChildren();
  const totalMs = performance.now() - t0;
  return {
    tokensMs,
    previewLoadMs,
    previewLayoutMs,
    previewTimedOut: !previewOk,
    criticTotalMs: critic.totalMs,
    criticBuildMs: critic.buildMs,
    criticTimedOutAtCap: critic.timedOutAtCap,
    criticExceedsProductTimeout: critic.exceedsProductTimeout,
    criticFindingCount: critic.findingCount,
    totalMs,
    longTasks: probe.stop(),
    memBefore,
    memAfter: memory(),
  };
}

function summarizeLongTasks(trials: { longTasks: LongTask[] }[]): {
  trialsWithLongTask: number;
  maxDuration: number;
  medianMaxDuration: number | null;
} {
  const maxes = trials.map((trial) =>
    trial.longTasks.reduce((max, task) => Math.max(max, task.duration), 0),
  );
  const withTask = maxes.filter((value) => value > 50);
  return {
    trialsWithLongTask: withTask.length,
    maxDuration: maxes.reduce((max, value) => Math.max(max, value), 0),
    medianMaxDuration: stats(maxes)?.median ?? null,
  };
}

function peakUsed(snaps: MemorySnap[]): number | null {
  const used = snaps.filter((snap): snap is { used: number; total: number } => snap !== null).map((snap) => snap.used);
  if (used.length === 0) return null;
  return Math.max(...used);
}

async function runTrials<T>(
  count: number,
  fn: () => T | Promise<T>,
): Promise<T[]> {
  const out: T[] = [];
  for (let i = 0; i < count; i += 1) {
    out.push(await fn());
    maybeGc();
    await new Promise((resolve) => window.setTimeout(resolve, 30));
  }
  return out;
}

async function measureSample(entry: ManifestEntry) {
  status(`fetch ${entry.file}`);
  const response = await fetch(`/${entry.file.replace(/\\/g, "/")}`);
  if (!response.ok) throw new Error(`fetch ${entry.file} -> ${response.status}`);
  const html = await response.text();

  status(`${entry.family} ${entry.kib}KiB warmup`);
  await measureCombined(html);
  maybeGc();

  status(`${entry.family} ${entry.kib}KiB tokens`);
  const tokenTrials = await runTrials(TRIALS, () => measureTokens(html));

  status(`${entry.family} ${entry.kib}KiB preview`);
  const previewTrials = await runTrials(TRIALS, () => measurePreview(html));

  status(`${entry.family} ${entry.kib}KiB critic`);
  const criticTrials = await runTrials(TRIALS, () => measureCritic(html));

  status(`${entry.family} ${entry.kib}KiB combined`);
  const combinedTrials = await runTrials(TRIALS, () => measureCombined(html));

  status(`${entry.family} ${entry.kib}KiB done`);

  return {
    entry,
    srcDocBytes: new TextEncoder().encode(artifactSrcDoc(html)).length,
    constants: {
      ARTIFACT_RENDER_CRITIC_TIMEOUT_MS,
      ARTIFACT_RENDER_CRITIC_SANDBOX,
      PREVIEW_CSP,
      previewSandbox: "",
      criticCapMs: CRITIC_CAP_MS,
      previewSize: { width: 700, height: 500 },
      trials: TRIALS,
    },
    isolated: {
      tokens: {
        stats: stats(tokenTrials.map((trial) => trial.ms)),
        missingCount: tokenTrials[0]?.missingCount ?? null,
        longTasks: summarizeLongTasks(tokenTrials),
        peakUsedJsHeap: peakUsed(tokenTrials.flatMap((trial) => [trial.memBefore, trial.memAfter])),
        samples: tokenTrials.map((trial) => trial.ms),
      },
      preview: {
        load: stats(previewTrials.map((trial) => trial.loadMs)),
        layout: stats(previewTrials.map((trial) => trial.layoutMs)),
        timedOutCount: previewTrials.filter((trial) => trial.timedOut).length,
        longTasks: summarizeLongTasks(previewTrials),
        peakUsedJsHeap: peakUsed(previewTrials.flatMap((trial) => [trial.memBefore, trial.memAfter])),
        samples: previewTrials.map((trial) => ({ loadMs: trial.loadMs, layoutMs: trial.layoutMs })),
      },
      critic: {
        total: stats(criticTrials.map((trial) => trial.totalMs)),
        build: stats(criticTrials.map((trial) => trial.buildMs)),
        timedOutAtCapCount: criticTrials.filter((trial) => trial.timedOutAtCap).length,
        exceedsProductTimeoutCount: criticTrials.filter((trial) => trial.exceedsProductTimeout).length,
        findingCount: criticTrials[0]?.findingCount ?? null,
        longTasks: summarizeLongTasks(criticTrials),
        peakUsedJsHeap: peakUsed(criticTrials.flatMap((trial) => [trial.memBefore, trial.memAfter])),
        samples: criticTrials.map((trial) => ({
          totalMs: trial.totalMs,
          buildMs: trial.buildMs,
          timedOutAtCap: trial.timedOutAtCap,
          exceedsProductTimeout: trial.exceedsProductTimeout,
        })),
      },
    },
    combined: {
      total: stats(combinedTrials.map((trial) => trial.totalMs)),
      tokens: stats(combinedTrials.map((trial) => trial.tokensMs)),
      previewLayout: stats(combinedTrials.map((trial) => trial.previewLayoutMs)),
      critic: stats(combinedTrials.map((trial) => trial.criticTotalMs)),
      criticExceedsProductTimeoutCount: combinedTrials.filter((trial) => trial.criticExceedsProductTimeout)
        .length,
      criticTimedOutAtCapCount: combinedTrials.filter((trial) => trial.criticTimedOutAtCap).length,
      longTasks: summarizeLongTasks(combinedTrials),
      peakUsedJsHeap: peakUsed(combinedTrials.flatMap((trial) => [trial.memBefore, trial.memAfter])),
      samples: combinedTrials.map((trial) => ({
        totalMs: trial.totalMs,
        tokensMs: trial.tokensMs,
        previewLayoutMs: trial.previewLayoutMs,
        criticTotalMs: trial.criticTotalMs,
        criticExceedsProductTimeout: trial.criticExceedsProductTimeout,
        criticTimedOutAtCap: trial.criticTimedOutAtCap,
      })),
    },
  };
}

declare global {
  interface Window {
    measureSample: typeof measureSample;
    measureReady: boolean;
    measureConstants: {
      ARTIFACT_RENDER_CRITIC_TIMEOUT_MS: number;
      ARTIFACT_RENDER_CRITIC_SANDBOX: string;
    };
  }
}

window.measureSample = measureSample;
window.measureReady = true;
window.measureConstants = {
  ARTIFACT_RENDER_CRITIC_TIMEOUT_MS,
  ARTIFACT_RENDER_CRITIC_SANDBOX,
};
status("harness ready");
