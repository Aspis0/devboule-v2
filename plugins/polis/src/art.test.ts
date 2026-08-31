import { describe, expect, it } from "vitest";
import { visualLevel, visualPurpose } from "./art";

describe("v1 visual classification seam", () => {
  it("keeps generic TSX files as houses instead of making every component a theater", () => {
    expect(visualPurpose("src/components/Panel.tsx")).toBe("house");
  });

  it("keeps v1 directory roles and real entrypoints ahead of extensions", () => {
    expect(visualPurpose("src/oracle/query.ts")).toBe("temple");
    expect(visualPurpose("src/agents/runner.ts")).toBe("fortress");
    expect(visualPurpose("src/main.tsx")).toBe("lighthouse");
  });

  it("uses v1 line-count thresholds for the five growth levels", () => {
    expect(visualLevel(200)).toBe(0);
    expect(visualLevel(201)).toBe(1);
    expect(visualLevel(600)).toBe(1);
    expect(visualLevel(601)).toBe(2);
    expect(visualLevel(1200)).toBe(2);
    expect(visualLevel(1201)).toBe(3);
    expect(visualLevel(2500)).toBe(3);
    expect(visualLevel(2501)).toBe(4);
  });
});
