// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  ARTIFACT_RENDER_CRITIC_MESSAGE_KIND,
  ARTIFACT_RENDER_CRITIC_SANDBOX,
  ARTIFACT_RENDER_CRITIC_SOURCE,
  ARTIFACT_RENDER_CRITIC_TIMEOUT_MS,
  ArtifactRenderCritic,
  buildArtifactMeasurementSrcDoc,
  contrastRatio,
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

afterEach(() => {
  document.body.replaceChildren();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

describe("artifact render critic pure helpers", () => {
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

  it("calculates WCAG contrast and recognizes both large-text thresholds", () => {
    expect(contrastRatio(BLACK, WHITE)).toBeCloseTo(21, 10);
    expect(contrastRatio({ r: 119, g: 119, b: 119 }, WHITE)).toBeCloseTo(4.478, 2);
    expect(isLargeScaleText(24, 400)).toBe(true);
    expect(isLargeScaleText(18.6666666667, 700)).toBe(true);
    expect(isLargeScaleText(18.666, 700)).toBe(false);
    expect(isLargeScaleText(18.6666666667, 400)).toBe(false);
  });

  it("rejects malformed result payloads", () => {
    expect(readArtifactRenderCriticResult(VALID_RESULT)).toEqual(VALID_RESULT);
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

    expect(readArtifactRenderCriticMessage(validEvent, frameWindow)).toEqual(VALID_RESULT);
    expect(readArtifactRenderCriticMessage(foreignEvent, frameWindow)).toBeNull();
    expect(readArtifactRenderCriticMessage(malformedEvent, frameWindow)).toBeNull();
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
      "Render checks found 1 finding.",
    );
    expect(container.textContent).toContain("Measured <code> at 1:1");
    await act(async () => root.unmount());
  });
});
