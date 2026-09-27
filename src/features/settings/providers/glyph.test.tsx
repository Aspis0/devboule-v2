// @vitest-environment happy-dom

// Our own kind marks per provider: the strip's marks for the three we draw,
// its generic agent mark otherwise — never a brand logo.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { ProviderGlyph } from "./ProviderGlyph";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

describe("ProviderGlyph", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function renderGlyph(providerId: string) {
    await act(async () => root.render(<ProviderGlyph providerId={providerId} />));
  }

  it("draws the strip's burst mark for claude", async () => {
    await renderGlyph("claude");
    expect(container.querySelector('[data-mark="burst"]')).not.toBeNull();
  });

  it("draws the strip's hex-dot mark for codex and its pi mark for pi", async () => {
    await renderGlyph("codex");
    expect(container.querySelector('[data-mark="hex-dot"]')).not.toBeNull();
    await renderGlyph("pi");
    expect(container.querySelector('[data-mark="pi"]')).not.toBeNull();
  });

  it("draws the generic agent mark, not a brand logo, for any other provider", async () => {
    await renderGlyph("grok");
    expect(container.querySelector('[data-mark="agent"]')).not.toBeNull();
    expect(container.querySelector('[data-mark="burst"]')).toBeNull();
    expect(container.querySelector('[data-mark="hex-dot"]')).toBeNull();
    expect(container.querySelector('[data-mark="pi"]')).toBeNull();
  });
});
