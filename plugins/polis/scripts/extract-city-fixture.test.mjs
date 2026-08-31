import { describe, expect, it } from "vitest";
import { createFixtureOverlays, extractCityFromSources } from "./extract-city-fixture.mjs";

describe("city fixture extractor", () => {
  it("emits files, districts, line counts, and directed weighted imports", () => {
    const city = extractCityFromSources([
      { path: "src/main.ts", source: 'import { app } from "./app";\n' },
      { path: "src/app.ts", source: "export const app = true;\n" },
      { path: "crates/demo/src/lib.rs", source: "use crate::config::Config;\n" },
      { path: "crates/demo/src/config.rs", source: "pub struct Config;\n" },
    ]);

    expect(city.files).toEqual([
      {
        id: "crates/demo/src/config.rs",
        path: "crates/demo/src/config.rs",
        lines: 1,
        district: "crates",
      },
      {
        id: "crates/demo/src/lib.rs",
        path: "crates/demo/src/lib.rs",
        lines: 1,
        district: "crates",
      },
      { id: "src/app.ts", path: "src/app.ts", lines: 1, district: "src" },
      { id: "src/main.ts", path: "src/main.ts", lines: 1, district: "src" },
    ]);
    expect(city.imports).toEqual([
      { from: "crates/demo/src/lib.rs", to: "crates/demo/src/config.rs", weight: 1 },
      { from: "src/main.ts", to: "src/app.ts", weight: 1 },
    ]);
  });

  it("adds labelled agent and finding fixture overlays without inventing a null-file position", () => {
    const graph = extractCityFromSources([
      { path: "plugins/polis/src/model.ts", source: "export interface City {}\n" },
      { path: "plugins/polis/src/main.ts", source: 'import { City } from "./model";\n' },
      { path: "plugins/polis/src/renderer.ts", source: "export const renderer = true;\n" },
      {
        path: "plugins/polis/scripts/extract-city-fixture.mjs",
        source: "export const fixture = true;\n",
      },
    ]);
    const overlays = createFixtureOverlays(graph.files);
    const fileIds = new Set(graph.files.map((file) => file.id));

    expect(overlays.agents).toHaveLength(6);
    expect(overlays.agents.filter((agent) => agent.fileId === null)).toHaveLength(1);
    expect(
      overlays.agents
        .filter((agent) => agent.fileId !== null)
        .every((agent) => fileIds.has(agent.fileId)),
    ).toBe(true);
    expect(overlays.findings).toHaveLength(3);
    expect(overlays.findings.every((finding) => finding.id.startsWith("fixture-"))).toBe(true);
    expect(overlays.findings.every((finding) => fileIds.has(finding.fileId))).toBe(true);
    expect(overlays.findings.map((finding) => finding.severity)).toEqual([
      "smoke",
      "fire",
      "inferno",
    ]);
  });
});
