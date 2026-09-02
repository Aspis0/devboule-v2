// @vitest-environment happy-dom

import { describe, expect, it, vi } from "vitest";
import { createFindingInspector, indexFindingsByFile } from "./findingInspector";

const ID = "a".repeat(64);
const file = {
  id: "src/secrets.ts",
  path: "src/secrets.ts",
  lines: 24,
  district: "src",
};
const finding = {
  id: ID,
  fileId: file.id,
  severity: "inferno" as const,
  rule: "secret-token",
  title: "<secret title>",
};

describe("finding inspector", () => {
  it("indexes fixture findings so a burning fixture building is inspectable", () => {
    const container = document.createElement("section");
    const index = new Map<string, typeof finding[]>();
    indexFindingsByFile(index, [finding]);
    const inspector = createFindingInspector(container, vi.fn());

    inspector.open(file, index.get(file.id) ?? []);

    expect(container.querySelector(".polis-finding-row")?.textContent).toContain(
      "<secret title>",
    );
    inspector.destroy();
  });

  it("renders file findings, fetches selected details, and withholds secret evidence", async () => {
    const container = document.createElement("section");
    const invoke = vi.fn().mockResolvedValue({
      id: ID,
      rule: finding.rule,
      severity: finding.severity,
      title: finding.title,
      source: "secrets",
      startLine: 4,
      endLine: 8,
      locations: [
        { startLine: 4, endLine: 4 },
        { startLine: 8, endLine: 8 },
      ],
    });
    const inspector = createFindingInspector(container, invoke);

    inspector.open(file, [finding]);

    expect(container.querySelector(".polis-inspector-file")?.textContent).toContain(
      "src/secrets.ts",
    );
    expect(container.querySelector(".polis-finding-row")?.textContent).toContain(
      "<secret title>",
    );
    (container.querySelector(".polis-finding-row") as HTMLButtonElement).click();
    await Promise.resolve();
    await Promise.resolve();

    expect(invoke).toHaveBeenCalledWith("finding.inspect", { id: ID }, expect.any(Number));
    expect(container.textContent).toContain("lines 4–8");
    expect(container.textContent).toContain("line 4");
    expect(container.textContent).toContain("line 8");
    expect(container.textContent).toContain("Detector: secrets");
    expect(container.textContent).toContain("Evidence is withheld");
    expect(container.querySelector("img")).toBeNull();
    inspector.destroy();
  });

  it("uses distinct copy for an expired finding and restores the hover readout on close", async () => {
    const container = document.createElement("section");
    const onClose = vi.fn();
    const invoke = vi.fn().mockRejectedValue(
      Object.assign(new Error("finding not found"), { code: "invalid_request" }),
    );
    const inspector = createFindingInspector(container, invoke, onClose);
    inspector.open(file, [finding]);
    (container.querySelector(".polis-finding-row") as HTMLButtonElement).click();
    await Promise.resolve();
    await Promise.resolve();

    expect(container.textContent).toContain("finding expired");
    (container.querySelector(".polis-inspector-close") as HTMLButtonElement).click();
    expect(onClose).toHaveBeenCalledWith(file);
    expect(container.textContent).toBe("");
    inspector.destroy();
  });

  it("keeps an empty building honest and distinguishes inspection failures", async () => {
    const container = document.createElement("section");
    const invoke = vi.fn();
    const inspector = createFindingInspector(container, invoke);
    inspector.open(file, []);
    expect(container.textContent).toContain("No findings for this building.");
    expect(container.textContent).toContain("Select a finding to inspect its lines.");

    const failures = [
      ["timeout", "Finding details timed out."],
      ["refusal", "Finding details were refused by the backend."],
      ["malformed_finding_inspection", "Finding details were malformed."],
    ] as const;
    for (const [failure, copy] of failures) {
      invoke.mockRejectedValueOnce(Object.assign(new Error(failure), { code: failure }));
      inspector.open(file, [finding]);
      (container.querySelector(".polis-finding-row") as HTMLButtonElement).click();
      await Promise.resolve();
      await Promise.resolve();
      expect(container.textContent).toContain(copy);
    }
    inspector.destroy();
  });

  it("does not paint an inspection result after pagehide", async () => {
    const container = document.createElement("section");
    const invoke = vi.fn();
    let resolveInspection!: (value: unknown) => void;
    invoke.mockReturnValue(
      new Promise((resolve) => {
        resolveInspection = resolve;
      }),
    );
    const inspector = createFindingInspector(container, invoke);
    inspector.open(file, [finding]);
    (container.querySelector(".polis-finding-row") as HTMLButtonElement).click();
    window.dispatchEvent(new Event("pagehide"));
    resolveInspection({
      id: ID,
      rule: finding.rule,
      severity: finding.severity,
      title: finding.title,
      source: "untested",
      startLine: 2,
      endLine: 2,
      locations: [{ startLine: 2, endLine: 2 }],
    });
    await Promise.resolve();
    await Promise.resolve();
    expect(container.textContent).toContain("Finding details: loading…");
    inspector.destroy();
  });

  it("invalidates an inspection when refreshed findings rebuild the live panel", async () => {
    const container = document.createElement("section");
    const invoke = vi.fn();
    let resolveInspection!: (value: unknown) => void;
    invoke.mockReturnValue(
      new Promise((resolve) => {
        resolveInspection = resolve;
      }),
    );
    const inspector = createFindingInspector(container, invoke);
    inspector.open(file, [finding]);
    (container.querySelector(".polis-finding-row") as HTMLButtonElement).click();
    const detachedDetail = container.querySelector(".polis-finding-detail") as HTMLElement;

    inspector.refreshFindings([finding]);
    const liveDetail = container.querySelector(".polis-finding-detail") as HTMLElement;
    resolveInspection({
      id: ID,
      rule: finding.rule,
      severity: finding.severity,
      title: finding.title,
      source: "untested",
      startLine: 3,
      endLine: 3,
      locations: [{ startLine: 3, endLine: 3 }],
    });
    await Promise.resolve();
    await Promise.resolve();

    expect(liveDetail.textContent).toBe("Select a finding for line details.");
    expect(detachedDetail.textContent).toBe("Finding details: loading…");
    inspector.destroy();
  });
});
