// @vitest-environment happy-dom

import { describe, expect, it, vi } from "vitest";
import {
  createHostFindingsReadout,
  FindingLayer,
  rendererFailedFindingsState,
  renderFindingsInCityStats,
  startFindingsScan,
} from "./findings";
import { pendingFindingsState } from "./hostBridge";

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

  it("keeps the failure through the host city-stats render path", () => {
    const element = document.createElement("div");
    const readout = createHostFindingsReadout(element);
    readout.setState({
      status: "failed",
      failure: "refusal",
      error: new Error("findings root unreadable"),
    });

    renderFindingsInCityStats(readout, {
      files: [{ id: "src/main.ts", path: "src/main.ts", lines: 1, district: "src" }],
      imports: [],
      agents: [],
      findings: [],
      dataSource: "host",
    });

    expect(element.textContent).toBe("Findings: scan refusal — findings root unreadable");
  });

  it("paints host scanning immediately when pending state is installed", () => {
    const element = document.createElement("div");
    const readout = createHostFindingsReadout(element);

    readout.setState(pendingFindingsState(), new Set());

    expect(element.textContent).toBe("Findings: scanning the workspace…");
  });

  it("has an honest renderer-failed state when the scan never starts", () => {
    const element = document.createElement("div");
    const readout = createHostFindingsReadout(element);

    readout.setState(rendererFailedFindingsState(), new Set());

    expect(element.textContent).toBe("Findings: renderer failed — scan not started");
  });

  it("does not apply a resolved scan after pagehide", async () => {
    const element = document.createElement("div");
    const readout = createHostFindingsReadout(element);
    let resolveScan!: (state: {
      status: "host";
      findings: [];
      scanned: true;
      completed: string[];
      failed: string[];
      scanMs: number;
      droppedFindings: number;
    }) => void;
    const refreshed = vi.fn();

    startFindingsScan(
      () =>
        new Promise((resolve) => {
          resolveScan = resolve;
        }),
      new Set(),
      readout,
      refreshed,
    );
    window.dispatchEvent(new Event("pagehide"));
    resolveScan({
      status: "host",
      findings: [],
      scanned: true,
      completed: [],
      failed: [],
      scanMs: 1,
      droppedFindings: 0,
    });
    await Promise.resolve();
    await Promise.resolve();

    expect(element.textContent).toBe("");
    expect(refreshed).not.toHaveBeenCalled();
  });
});
