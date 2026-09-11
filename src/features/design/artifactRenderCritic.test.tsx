// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { Window as HappyWindow } from "happy-dom";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ARTIFACT_CSP } from "./artifactCsp";
import {
  ARTIFACT_RENDER_CRITIC_CSP,
  ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
  ARTIFACT_RENDER_CRITIC_SANDBOX,
  ARTIFACT_RENDER_CRITIC_SOURCE,
  ARTIFACT_RENDER_CRITIC_TIMEOUT_MS,
  ArtifactRenderCritic,
  buildArtifactMeasurementSrcDoc,
  contrastRatio,
  findingHeadline,
  isLargeScaleText,
  readArtifactRenderCriticMessage,
  readArtifactRenderCriticResult,
  stripArtifactScriptsAndHandlers,
} from "./artifactRenderCritic";

const BLACK = { r: 0, g: 0, b: 0 };
const WHITE = { r: 255, g: 255, b: 255 };

const VALID_RESULT = {
  kind: ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
  source: ARTIFACT_RENDER_CRITIC_SOURCE,
  version: 1,
  findings: [
    {
      kind: "contrast",
      count: 1,
      samples: [{ label: "<code>", ratio: 1, minimum: 4.5, fontSizePx: 16 }],
    },
  ],
} as const;

const CLEAN_RESULT = { ...VALID_RESULT, findings: [] };

function runAssembledMeasurement(html: string, setup?: (measurementWindow: HappyWindow) => void) {
  const srcDoc = buildArtifactMeasurementSrcDoc(html);
  const scriptMatch = srcDoc.match(/<script>([\s\S]*?)<\/script>/);
  if (scriptMatch === null) throw new Error("Measurement script was not assembled");
  const measurementWindow = new HappyWindow({ url: "http://measurement.test/" });
  let posted: unknown;
  const parent = { postMessage: (message: unknown) => (posted = message) };
  Object.defineProperty(measurementWindow, "parent", {
    configurable: true,
    value: parent,
  });
  measurementWindow.document.write(srcDoc.replace(scriptMatch[0], ""));
  setup?.(measurementWindow);
  measurementWindow.eval(scriptMatch[1]);
  measurementWindow.happyDOM.close();
  const result = readArtifactRenderCriticResult(posted);
  if (result === null) throw new Error("Measurement script did not post a valid result");
  return result;
}

function setRect(element: object, width: number, height: number, left = 0, top = 0) {
  Object.defineProperty(element, "getBoundingClientRect", {
    configurable: true,
    value: () => ({
      bottom: top + height,
      height,
      left,
      right: left + width,
      top,
      width,
      x: left,
      y: top,
    }),
  });
}

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("artifact render critic pure helpers", () => {
  it("derives its policy from the canvas one by swapping exactly the script directive", () => {
    // The critic's policy is built with a `.replace` on ARTIFACT_CSP, and a
    // `.replace` that matches nothing returns the string unchanged instead of
    // failing. Were the base ever written so the substring stopped matching,
    // the critic would silently inherit `script-src 'none'`, its measurement
    // script would never run, and the failure would surface as a timeout
    // rather than as a broken build. These assertions are what make that
    // silence audible.
    expect(ARTIFACT_RENDER_CRITIC_CSP).not.toBe(ARTIFACT_CSP);
    expect(ARTIFACT_RENDER_CRITIC_CSP).toContain("script-src 'unsafe-inline'");
    expect(ARTIFACT_RENDER_CRITIC_CSP).not.toContain("script-src 'none'");
    expect(
      ARTIFACT_RENDER_CRITIC_CSP.replace("script-src 'unsafe-inline'", "script-src 'none'"),
    ).toBe(ARTIFACT_CSP);
  });

  it("strips script elements and on* attributes while preserving other markup", () => {
    const html =
      '<script>window.compromised = true;</script><button data-label="a > b" onclick="bad()" onfocus=bad class="button">Run</button><p onload=bad>Text</p>';

    expect(stripArtifactScriptsAndHandlers(html)).toBe(
      '<button data-label="a > b" class="button">Run</button><p>Text</p>',
    );
  });

  it("removes an unclosed script through the end of the artifact", () => {
    expect(stripArtifactScriptsAndHandlers("<main>Before</main><script>never run")).toBe(
      "<main>Before</main>",
    );
  });

  it("does not treat a script-looking string inside a style element as a script element", () => {
    const html = '<style>.label::before { content: "<script>"; }</style><p onload=bad>Text</p>';

    expect(stripArtifactScriptsAndHandlers(html)).toBe(
      '<style>.label::before { content: "<script>"; }</style><p>Text</p>',
    );
  });

  it("assembles a standards-mode document with one wrapped measurement script", () => {
    const artifact =
      '<!doctype html><html><head><title>Measured</title></head><body><script>alert("bad")</script><button onclick="bad()">Run</button></body></html>';
    const srcDoc = buildArtifactMeasurementSrcDoc(artifact);
    const scriptTags = srcDoc.match(/<script\b/gi) ?? [];
    const head = srcDoc.match(/<head\b[^>]*>[\s\S]*?<\/head>/i)?.[0];
    const scriptStart = srcDoc.indexOf("<script>");
    const scriptEnd = srcDoc.indexOf("</script>", scriptStart);

    expect(srcDoc.startsWith("<!doctype html>")).toBe(true);
    expect(scriptTags).toHaveLength(1);
    expect(scriptStart).toBeGreaterThan(-1);
    expect(scriptEnd).toBeGreaterThan(scriptStart);
    expect(srcDoc.slice(scriptStart, scriptEnd)).toContain(
      'const SOURCE = "devboule-artifact-render-critic"',
    );
    expect(head).toContain('<meta http-equiv="Content-Security-Policy"');
    expect(srcDoc.indexOf("<head")).toBeLessThan(srcDoc.indexOf("script-src"));
    expect(srcDoc).toContain("<button>Run</button>");
    expect(srcDoc).not.toContain('alert("bad")');
    expect(srcDoc).not.toContain("onclick");
    expect(srcDoc).not.toContain(`sandbox="${ARTIFACT_RENDER_CRITIC_SANDBOX}"`);
  });

  it("adds a head for markup with neither a doctype nor a head", () => {
    const srcDoc = buildArtifactMeasurementSrcDoc('<main onclick="bad()">Measured</main>');
    const head = srcDoc.match(/<head\b[^>]*>[\s\S]*?<\/head>/i)?.[0];
    const scriptTags = srcDoc.match(/<script\b/gi) ?? [];

    expect(srcDoc.startsWith("<!doctype")).toBe(false);
    expect(srcDoc.startsWith("<head>")).toBe(true);
    expect(head).toContain('<meta http-equiv="Content-Security-Policy"');
    expect(scriptTags).toHaveLength(1);
    expect(srcDoc).toContain("<main>Measured</main>");
    expect(srcDoc).not.toContain("onclick");
  });

  it("measures the page height in the same pass as the findings", () => {
    const result = runAssembledMeasurement("<main><p>Text</p></main>");
    expect(typeof result.contentHeight).toBe("number");
    expect(result.contentHeight).toBeGreaterThanOrEqual(0);
  });

  it("measures native controls with their associated label target", () => {
    const result = runAssembledMeasurement(
      `<main>
        <label id="check-label"><input id="check" type="checkbox"></label>
        <label for="radio-one">Radio one</label><input id="radio-one" type="radio">
        <label for="radio-two">Radio two</label><input id="radio-two" type="radio">
        <label for="radio-three">Radio three</label><input id="radio-three" type="radio">
      </main>`,
      (measurementWindow) => {
        const checkLabel = measurementWindow.document.querySelector("#check-label");
        const check = measurementWindow.document.querySelector("#check");
        const radioOneLabel = measurementWindow.document.querySelector('label[for="radio-one"]');
        const radioOne = measurementWindow.document.querySelector("#radio-one");
        const radioTwoLabel = measurementWindow.document.querySelector('label[for="radio-two"]');
        const radioTwo = measurementWindow.document.querySelector("#radio-two");
        const radioThreeLabel = measurementWindow.document.querySelector(
          'label[for="radio-three"]',
        );
        const radioThree = measurementWindow.document.querySelector("#radio-three");
        if (
          checkLabel === null ||
          check === null ||
          radioOneLabel === null ||
          radioOne === null ||
          radioTwoLabel === null ||
          radioTwo === null ||
          radioThreeLabel === null ||
          radioThree === null
        ) {
          throw new Error("Native control fixture did not mount");
        }
        setRect(checkLabel, 350, 68);
        setRect(check, 1, 1);
        setRect(radioOneLabel, 220, 64);
        setRect(radioOne, 13, 13);
        setRect(radioTwoLabel, 220, 64);
        setRect(radioTwo, 13, 13);
        setRect(radioThreeLabel, 220, 64);
        setRect(radioThree, 13, 13);
      },
    );

    expect(result.findings.some((finding) => finding.kind === "pointer-target")).toBe(false);
  });

  it("reports a bare undersized native button", () => {
    const result = runAssembledMeasurement(
      '<button id="small" type="button">Small</button>',
      (measurementWindow) => {
        const button = measurementWindow.document.querySelector("#small");
        if (button === null) throw new Error("Button fixture did not mount");
        setRect(button, 12, 12);
      },
    );
    const finding = result.findings.find((candidate) => candidate.kind === "pointer-target");

    expect(finding).toMatchObject({ kind: "pointer-target", count: 1 });
  });

  it("measures authored focus contrast but does not report 3.247:1", () => {
    const lowContrast = runAssembledMeasurement(`
      <style>
        body { background-color: #f7f8f4; }
        button:focus-visible { outline: 3px solid #f0a187; }
      </style>
      <button>Sign in</button>
    `);
    const lowFinding = lowContrast.findings.find(
      (finding) => finding.kind === "focus-indicator" && finding.reason === "low-contrast",
    );
    expect(lowFinding).toMatchObject({ kind: "focus-indicator", reason: "low-contrast", count: 1 });
    if (lowFinding?.kind === "focus-indicator") {
      expect(lowFinding.samples[0]?.ratio).toBeCloseTo(1.939, 3);
    }

    const passing = runAssembledMeasurement(`
      <style>
        :root { --focus: #b88428; }
        body { background-color: #fffdfa; }
        button:focus-visible { outline: 3px solid var(--focus); }
      </style>
      <button>System</button>
    `);
    expect(
      passing.findings.some(
        (finding) => finding.kind === "focus-indicator" && finding.reason === "low-contrast",
      ),
    ).toBe(false);
  });

  it("resolves a focus color per matched element", () => {
    const result = runAssembledMeasurement(`
      <style>
        body { background-color: #f7f8f4; }
        button:focus-visible { outline: 3px solid currentColor; }
      </style>
      <div><button style="color: #001133">Light</button></div>
      <div><button style="color: #f0a187">Dark</button></div>
    `);
    const finding = result.findings.find(
      (candidate) => candidate.kind === "focus-indicator" && candidate.reason === "low-contrast",
    );

    expect(finding).toMatchObject({ kind: "focus-indicator", reason: "low-contrast", count: 1 });
    if (finding?.kind === "focus-indicator") {
      expect(finding.samples[0]?.label).toContain('"Dark"');
      expect(finding.samples[0]?.ratio).toBeCloseTo(1.939, 3);
    }
  });

  it("reports an outline removed without a replacement only", () => {
    const removed = runAssembledMeasurement(`
      <style>button:focus-visible { outline: none; }</style>
      <button>Remove ring</button>
    `);
    expect(removed.findings).toContainEqual(
      expect.objectContaining({ kind: "focus-indicator", reason: "removed", count: 1 }),
    );

    const replacement = runAssembledMeasurement(`
      <style>button:focus-visible { outline: none; background: #dcebe5; }</style>
      <button>Replacement</button>
    `);
    expect(
      replacement.findings.some(
        (finding) => finding.kind === "focus-indicator" && finding.reason === "removed",
      ),
    ).toBe(false);
  });

  it("suppresses removed when another rule supplies a focus ring", () => {
    const result = runAssembledMeasurement(`
      <style>
        .btn:focus-visible, .btn { outline: none; font-weight: 600; }
        .btn:focus-visible { outline: 3px solid #001133; }
      </style>
      <button class="btn">Save</button>
    `);

    expect(result.findings).toEqual([]);
  });

  it("treats auto outlines and supported width units as unknown visible indicators", () => {
    const auto = runAssembledMeasurement(
      `
      <style>
        .auto:focus-visible, .auto { font-weight: 600; }
        .auto:focus-visible { outline: auto; }
        .style-auto:focus-visible, .style-auto { font-weight: 600; }
        .style-auto:focus-visible { outline-style: auto; outline-width: 2em; }
        .rem:focus-visible, .rem { font-weight: 600; }
        .rem:focus-visible { outline-width: 1rem; outline-style: solid; outline-color: #001133; }
        .percent:focus-visible, .percent { font-weight: 600; }
        .percent:focus-visible { outline-width: 2%; outline-style: solid; outline-color: #001133; }
      </style>
      <button class="auto">Auto</button>
      <button class="style-auto">Style auto</button>
      <button class="rem">Rem</button>
      <button class="percent">Percent</button>
    `,
      (measurementWindow) => {
        const style = (values: Record<string, string>) => ({
          getPropertyValue: (property: string) => values[property] ?? "",
          item: (index: number) => Object.keys(values)[index] ?? "",
          length: Object.keys(values).length,
        });
        const rule = (selectorText: string, values: Record<string, string>) => ({
          cssRules: [],
          selectorText,
          style: style(values),
        });
        Object.defineProperty(measurementWindow.document, "styleSheets", {
          configurable: true,
          value: [
            {
              cssRules: [
                rule(".auto:focus-visible, .auto", { outline: "auto" }),
                rule(".style-auto:focus-visible, .style-auto", {
                  "outline-style": "auto",
                  "outline-width": "2em",
                }),
                rule(".rem:focus-visible, .rem", {
                  "outline-color": "#001133",
                  "outline-style": "solid",
                  "outline-width": "1rem",
                }),
                rule(".percent:focus-visible, .percent", {
                  "outline-color": "#001133",
                  "outline-style": "solid",
                  "outline-width": "2%",
                }),
              ],
            },
          ],
        });
      },
    );

    expect(auto.findings.some((finding) => finding.kind === "focus-indicator")).toBe(false);
  });

  it("gives removed precedence over always-on for the same element", () => {
    const result = runAssembledMeasurement(`
      <style>.btn:focus-visible, .btn { outline: none; font-weight: 600; }</style>
      <button class="btn">Save</button>
    `);
    const focusFindings = result.findings.filter((finding) => finding.kind === "focus-indicator");

    expect(focusFindings).toContainEqual(
      expect.objectContaining({ kind: "focus-indicator", reason: "removed", count: 1 }),
    );
    expect(focusFindings.some((finding) => finding.reason === "always-on")).toBe(false);
  });

  it("does not use aria-labelledby as a pointer-target label", () => {
    const result = runAssembledMeasurement(
      `<span id="label" style="display:inline-block;width:200px;padding:20px">Notifications</span>
       <input class="hidden-input" type="checkbox" aria-labelledby="label">`,
      (measurementWindow) => {
        const input = measurementWindow.document.querySelector("input");
        if (input === null) throw new Error("ARIA-labelledby fixture did not mount");
        setRect(input, 1, 1);
      },
    );
    const finding = result.findings.find((candidate) => candidate.kind === "pointer-target");

    expect(finding).toMatchObject({ kind: "pointer-target", count: 1 });
  });

  it("suppresses always-on when another focus rule supplies an indicator", () => {
    const suppliedIndicator = runAssembledMeasurement(`
      <style>
        .btn:focus-visible { outline: 3px solid #001133; }
        .btn, .btn:focus-visible { font-weight: 600; }
      </style>
      <button class="btn" style="padding:12px 20px">Save</button>
    `);
    expect(suppliedIndicator.findings).toEqual([]);
  });

  it("reports a static aria-current collision but not a hover/focus selector pair", () => {
    const collision = runAssembledMeasurement(`
      <style>
        .nav a:hover, .nav a:focus-visible, .nav a[aria-current="page"] {
          background: #f0f0f0;
          outline: none;
        }
      </style>
      <nav class="nav"><a href="#" aria-current="page">New simulation</a></nav>
    `);
    expect(collision.findings).toContainEqual(
      expect.objectContaining({ kind: "focus-indicator", reason: "always-on", count: 1 }),
    );

    const transient = runAssembledMeasurement(`
      <style>a:hover, a:focus-visible { outline: 3px solid #000; }</style>
      <a href="#">Transient</a>
    `);
    expect(transient.findings.some((finding) => finding.kind === "focus-indicator")).toBe(false);
  });

  it("calculates WCAG contrast and recognizes both large-text thresholds", () => {
    expect(contrastRatio(BLACK, WHITE)).toBeCloseTo(21, 10);
    expect(contrastRatio({ r: 119, g: 119, b: 119 }, WHITE)).toBeCloseTo(4.478, 2);
    expect(isLargeScaleText(24, 400)).toBe(true);
    expect(isLargeScaleText(18.6666666667, 700)).toBe(true);
    expect(isLargeScaleText(18.666, 700)).toBe(false);
    expect(isLargeScaleText(18.6666666667, 400)).toBe(false);
  });

  it("keeps a measured page height but drops an invalid one", () => {
    expect(readArtifactRenderCriticResult({ ...VALID_RESULT, contentHeight: 3600 })).toEqual({
      ...VALID_RESULT,
      structure: [],
      contentHeight: 3600,
    });
    // A malformed height degrades to unmeasured; it never hides the findings.
    expect(
      readArtifactRenderCriticResult({ ...VALID_RESULT, contentHeight: -1 })?.contentHeight,
    ).toBeUndefined();
    expect(
      readArtifactRenderCriticResult({ ...VALID_RESULT, contentHeight: "tall" })?.contentHeight,
    ).toBeUndefined();
  });

  it("rejects malformed result payloads", () => {
    expect(readArtifactRenderCriticResult(VALID_RESULT)).toEqual({
      ...VALID_RESULT,
      structure: [],
    });
    expect(readArtifactRenderCriticResult({ ...VALID_RESULT, version: 2 })).toBeNull();
    expect(
      readArtifactRenderCriticResult({
        ...VALID_RESULT,
        findings: [{ ...VALID_RESULT.findings[0], count: "1" }],
      }),
    ).toBeNull();
    expect(
      readArtifactRenderCriticResult({
        ...VALID_RESULT,
        findings: [{ ...VALID_RESULT.findings[0], samples: [{ label: "<code>" }] }],
      }),
    ).toBeNull();
    expect(
      readArtifactRenderCriticResult({
        ...VALID_RESULT,
        findings: [{ ...VALID_RESULT.findings[0], samples: [] }],
      }),
    ).toBeNull();
  });

  it("accepts only a validated result from the measurement frame window", () => {
    const frameWindow = {} as Window;
    const foreignWindow = {} as Window;
    const validEvent = new MessageEvent("message", {
      data: VALID_RESULT,
      source: frameWindow,
    });
    const foreignEvent = new MessageEvent("message", {
      data: VALID_RESULT,
      source: foreignWindow,
    });
    const malformedEvent = new MessageEvent("message", {
      data: { ...VALID_RESULT, findings: "not-an-array" },
      source: frameWindow,
    });

    expect(readArtifactRenderCriticMessage(validEvent, frameWindow)).toEqual({
      ...VALID_RESULT,
      structure: [],
    });
    expect(readArtifactRenderCriticMessage(foreignEvent, frameWindow)).toBeNull();
    expect(readArtifactRenderCriticMessage(malformedEvent, frameWindow)).toBeNull();
  });
});

describe("artifact render critic headline", () => {
  it("names each measured group instead of totalling different kinds", () => {
    expect(
      findingHeadline({
        kind: "contrast",
        count: 30,
        samples: [{ label: "<a>", ratio: 3.93, minimum: 4.5, fontSizePx: 14 }],
      }),
    ).toBe("30 low-contrast texts");
    expect(
      findingHeadline({
        kind: "pointer-target",
        count: 7,
        samples: [{ label: "<button>", width: 20, height: 20 }],
      }),
    ).toBe("7 small targets");
    expect(
      findingHeadline({
        kind: "contrast",
        count: 1,
        samples: [{ label: "<code>", ratio: 1, minimum: 4.5, fontSizePx: 16 }],
      }),
    ).toBe("1 low-contrast text");
    expect(
      findingHeadline({
        kind: "pointer-target",
        count: 1,
        samples: [{ label: "<button>", width: 12, height: 12 }],
      }),
    ).toBe("1 small target");
    expect(
      findingHeadline({
        kind: "overflow",
        count: 2,
        samples: [{ label: "<div>", scrollWidth: 300, clientWidth: 200 }],
      }),
    ).toBe("2 overflowing elements");
    expect(
      findingHeadline({
        kind: "focus-indicator",
        reason: "removed",
        count: 1,
        samples: [{ label: "<button>", selector: "button:focus-visible" }],
      }),
    ).toBe("1 removed focus indicator");
  });
});

describe("artifact render critic lifecycle", () => {
  it("removes the pending frame, listener, and timeout when the artifact changes or unmounts", async () => {
    vi.useFakeTimers();
    const addEventListener = vi.spyOn(window, "addEventListener");
    const removeEventListener = vi.spyOn(window, "removeEventListener");
    const clearTimeout = vi.spyOn(window, "clearTimeout");
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<ArtifactRenderCritic html="<main>First</main>" />);
    });
    const firstFrame = document.querySelector<HTMLIFrameElement>(
      ".design-artifact-measurement-frame",
    );
    const messageHandler = addEventListener.mock.calls.find(([type]) => type === "message")?.[1];
    if (firstFrame === null || messageHandler === undefined) {
      throw new Error("Measurement plumbing did not mount");
    }
    expect(firstFrame.getAttribute("sandbox")).toBe(ARTIFACT_RENDER_CRITIC_SANDBOX);

    await act(async () => {
      root.render(<ArtifactRenderCritic html="<main>Second</main>" />);
    });
    const secondFrame = document.querySelector<HTMLIFrameElement>(
      ".design-artifact-measurement-frame",
    );
    expect(firstFrame.isConnected).toBe(false);
    expect(secondFrame).not.toBeNull();
    expect(removeEventListener).toHaveBeenCalledWith("message", messageHandler);
    expect(clearTimeout).toHaveBeenCalled();

    await act(async () => root.unmount());
    expect(secondFrame?.isConnected).toBe(false);
    vi.advanceTimersByTime(ARTIFACT_RENDER_CRITIC_TIMEOUT_MS);
    expect(container.querySelector('[role="status"]')).toBeNull();
  });

  it("renders the card only after a valid frame result arrives", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(<ArtifactRenderCritic html="<main>Measured</main>" />);
    });
    const frame = document.querySelector<HTMLIFrameElement>(".design-artifact-measurement-frame");
    if (frame === null || frame.contentWindow === null) {
      throw new Error("Measurement frame did not mount");
    }

    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: VALID_RESULT,
          source: frame.contentWindow,
        }),
      );
    });

    expect(container.querySelector('[role="status"]')?.textContent).toContain(
      "Render checks found 1 low-contrast text.",
    );
    expect(container.textContent).toContain("Measured <code> at 1:1");
    await act(async () => root.unmount());
  });

  it("names each measured group in the headline instead of totalling kinds", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(<ArtifactRenderCritic html="<main>Measured</main>" />);
    });
    const frame = document.querySelector<HTMLIFrameElement>(".design-artifact-measurement-frame");
    if (frame === null || frame.contentWindow === null) {
      throw new Error("Measurement frame did not mount");
    }

    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: {
            kind: ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
            source: ARTIFACT_RENDER_CRITIC_SOURCE,
            version: 1,
            findings: [
              {
                kind: "contrast",
                count: 30,
                samples: [{ label: "<a>", ratio: 3.93, minimum: 4.5, fontSizePx: 14 }],
              },
              {
                kind: "pointer-target",
                count: 7,
                samples: [{ label: "<button>", width: 20, height: 20 }],
              },
            ],
          },
          source: frame.contentWindow,
        }),
      );
    });

    expect(container.querySelector('[role="status"]')?.textContent).toContain(
      "Render checks found 30 low-contrast texts, 7 small targets.",
    );
    await act(async () => root.unmount());
  });

  it("reports a timeout after the check settles without confusing it with pending work", async () => {
    vi.useFakeTimers();
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);

    await act(async () => {
      root.render(<ArtifactRenderCritic html="<main>Slow</main>" />);
    });
    expect(container.querySelector('[role="status"]')).toBeNull();

    await act(async () => {
      vi.advanceTimersByTime(ARTIFACT_RENDER_CRITIC_TIMEOUT_MS);
    });

    expect(container.querySelector('[role="status"]')?.textContent).toBe(
      "This artifact was too slow to check; the render check did not run.",
    );
    await act(async () => root.unmount());
  });

  it("keeps a completed clean check quiet", async () => {
    const container = document.createElement("div");
    document.body.append(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(<ArtifactRenderCritic html="<main>Clean</main>" />);
    });
    const frame = document.querySelector<HTMLIFrameElement>(".design-artifact-measurement-frame");
    if (frame === null || frame.contentWindow === null) {
      throw new Error("Measurement frame did not mount");
    }

    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", {
          data: CLEAN_RESULT,
          source: frame.contentWindow,
        }),
      );
    });

    expect(container.querySelector('[role="status"]')).toBeNull();
    await act(async () => root.unmount());
  });
});
