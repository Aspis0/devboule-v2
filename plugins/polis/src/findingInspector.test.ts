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
    const inspectionFailures: unknown[] = [];
    const invoke = vi.fn((method: string) => {
      if (method === "oracle.search") {
        return Promise.resolve({ query: file.path, results: [] });
      }
      return Promise.reject(inspectionFailures.shift());
    });
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
      inspectionFailures.push(Object.assign(new Error(failure), { code: failure }));
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

  it("renders citation rows, tags this file, and only resolved paths are buttons", async () => {
    const other = {
      id: "crates/oracle-core/src/lib.rs",
      path: "crates/oracle-core/src/lib.rs",
      lines: 80,
      district: "crates",
    };
    const invoke = vi.fn().mockImplementation((method: string) => {
      if (method === "oracle.search") {
        return Promise.resolve({
          query: file.path,
          results: [
            {
              path: file.path,
              startLine: 43,
              endLine: 88,
              focusStartLine: 51,
              focusEndLine: 57,
              symbol: "OraclePanel",
              match: "dense+reranked",
            },
            {
              path: other.path,
              startLine: 1,
              endLine: 4,
            },
            {
              path: "docs/missing.md",
              startLine: 0,
              endLine: 0,
            },
          ],
        });
      }
      return Promise.resolve(null);
    });
    const openFile = vi.fn();
    const container = document.createElement("section");
    const inspector = createFindingInspector(container, invoke, () => undefined, {
      resolveFile: (path) => (path === file.path ? file : path === other.path ? other : null),
      openFile,
    });
    inspector.open(file, [finding]);
    await Promise.resolve();
    await Promise.resolve();

    expect(container.textContent).toContain("Oracle pointers");
    expect(container.textContent).toContain(
      "Ranked by similarity to this file's path. Oracle points, it does not answer.",
    );
    expect(container.textContent).toContain("#01");
    expect(container.textContent).toContain("this file");
    expect(container.textContent).toContain("lines 43–88");
    expect(container.textContent).toContain("start at 51–57");
    expect(container.textContent).toContain("symbol OraclePanel");
    expect(container.textContent).toContain("match dense+reranked");
    expect(container.textContent).toContain("lines unknown");
    expect(container.textContent).not.toContain("score");

    const buttons = [...container.querySelectorAll(".polis-oracle-citations button")];
    expect(buttons).toHaveLength(2);
    const missing = [...container.querySelectorAll(".polis-oracle-citation-plain")];
    expect(missing.some((node) => node.textContent?.includes("docs/missing.md"))).toBe(true);
    expect(missing.some((node) => node.tagName === "BUTTON")).toBe(false);

    buttons[1].dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(openFile).toHaveBeenCalledWith(other);
    inspector.destroy();
  });

  it("uses indexState to distinguish empty Oracle results", async () => {
    const cases: Array<[string | undefined, string]> = [
      ["idle", "No Oracle index for this workspace yet — build it in Settings › Oracle."],
      ["indexing", "Oracle is still indexing this workspace."],
      ["error", "Oracle's index is in an error state."],
      ["ready", "No spans matched this file."],
      [undefined, "No spans matched this file."],
    ];
    for (const [indexState, copy] of cases) {
      const invoke = vi.fn().mockResolvedValue({
        query: file.path,
        results: [],
        ...(indexState === undefined ? {} : { indexState }),
      });
      const container = document.createElement("section");
      const inspector = createFindingInspector(container, invoke);
      inspector.open(file, []);
      await Promise.resolve();
      await Promise.resolve();
      expect(container.textContent).toContain(copy);
      inspector.destroy();
    }

    const failures = [
      ["timeout", "Oracle pointers unavailable: the search timed out."],
      ["busy", "Oracle pointers unavailable: another search is still running."],
      ["invalid_request", "Oracle pointers unavailable: the host rejected the query."],
      ["capability_not_supported", "Oracle pointers unavailable: the host refused the request."],
      ["invalid_response", "Oracle pointers unavailable: the host returned an invalid response."],
    ] as const;
    for (const [code, copy] of failures) {
      const invoke = vi.fn().mockRejectedValue(Object.assign(new Error(code), { code }));
      const container = document.createElement("section");
      const inspector = createFindingInspector(container, invoke);
      inspector.open(file, []);
      await Promise.resolve();
      await Promise.resolve();
      expect(container.textContent).toContain(copy);
      inspector.destroy();
    }
  });

  it("invalidates citations when refreshed findings rebuild the live panel", async () => {
    const container = document.createElement("section");
    const invoke = vi.fn();
    let resolveCitations!: (value: unknown) => void;
    invoke.mockImplementation((method: string) => {
      if (method === "oracle.search") {
        return new Promise((resolve) => {
          resolveCitations = resolve;
        });
      }
      return Promise.resolve(null);
    });
    const inspector = createFindingInspector(container, invoke);
    inspector.open(file, [finding]);
    await Promise.resolve();
    const detached = container.querySelector(".polis-oracle-citations-body") as HTMLElement;

    inspector.refreshFindings([finding]);
    const live = container.querySelector(".polis-oracle-citations-body") as HTMLElement;
    resolveCitations({
      query: file.path,
      results: [{ path: file.path, startLine: 9, endLine: 9 }],
    });
    await Promise.resolve();
    await Promise.resolve();

    expect(live.textContent).toBe("Oracle pointers: searching…");
    expect(detached.textContent).toBe("Oracle pointers: searching…");
    inspector.destroy();
  });
});
