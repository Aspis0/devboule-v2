import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { StripKindMark } from "./StripKindMark";

describe("StripKindMark", () => {
  it("draws a burst for claude", () => {
    expect(renderToStaticMarkup(<StripKindMark kind="claude" />)).toContain('data-mark="burst"');
  });

  it("draws a hex-dot for codex", () => {
    expect(renderToStaticMarkup(<StripKindMark kind="codex" />)).toContain('data-mark="hex-dot"');
  });

  it("draws pi as its own letter", () => {
    const html = renderToStaticMarkup(<StripKindMark kind="pi" />);
    expect(html).toContain('data-mark="pi"');
    expect(html).toContain("π");
  });

  it("draws a terminal glyph for terminals", () => {
    expect(renderToStaticMarkup(<StripKindMark kind="terminal" />)).toContain(
      'data-mark="terminal"',
    );
  });

  it("falls back to the generic agent mark for acp and anything unknown", () => {
    expect(renderToStaticMarkup(<StripKindMark kind="acp" />)).toContain('data-mark="agent"');
    expect(renderToStaticMarkup(<StripKindMark kind="terminal" />)).not.toContain(
      'data-mark="agent"',
    );
  });

  it("stays out of the accessible name", () => {
    expect(renderToStaticMarkup(<StripKindMark kind="claude" />)).toContain('aria-hidden="true"');
  });
});
