import { describe, expect, it } from "vitest";
import { extractCityFromSources } from "./extract-city-fixture.mjs";

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
});
