// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import { createHostFindingsReadout, FindingLayer } from "./findings";

describe("FindingLayer", () => {
  it("replaces findings without leaking or double-drawing views", () => {
    const layer = new FindingLayer(
      new Map([["src/main.ts", { worldX: 12, worldY: 24, height: 8 }]]),
    );
    const finding = {
      id: "a".repeat(64),
      fileId: "src/main.ts",
      severity: "fire" as const,
      rule: "rule-a",
      title: "A finding",
    };

    layer.setFindings([finding]);
    expect(layer.root.children).toHaveLength(1);
    const firstView = layer.root.children[0];

    layer.setFindings([finding]);
    expect(layer.root.children).toHaveLength(1);
    expect(layer.root.children[0]).not.toBe(firstView);

    layer.setFindings([]);
    expect(layer.root.children).toHaveLength(0);
  });

  it("keeps a failed findings scan visible across later city-stat renders", () => {
    const element = document.createElement("div");
    const readout = createHostFindingsReadout(element);
    readout.setState({
      status: "failed",
      failure: "timeout",
      error: new Error("timed out: waiting for a plugin reply"),
    });
    readout.render(new Set());
    readout.render(new Set(["src/main.ts"]));
    expect(element.textContent).toBe(
      "Findings: scan timeout — timed out: waiting for a plugin reply",
    );
  });
});
