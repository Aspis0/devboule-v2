// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { FileTab, IndexedFile } from "../../types/ipc";

const mocks = vi.hoisted(() => ({
  oracleFiles: vi.fn(),
}));

vi.mock("../../lib/tauri", () => ({
  oracleFiles: mocks.oracleFiles,
}));

import {
  loadRepositoryLayers,
  MAX_DESIGN_LAYERS,
  MAX_ORACLE_FILE_PAGES,
  ORACLE_FILE_PAGE_SIZE,
} from "./repositoryLayers";

function indexedFile(path: string): IndexedFile {
  return { path, chunks: 1, updated_at: "2026-09-05T00:00:00Z" };
}

beforeEach(() => mocks.oracleFiles.mockReset());

describe("repository-derived design layers", () => {
  it("pages past the first 50 files to find the 201st file", async () => {
    const firstFourPages = Array.from({ length: ORACLE_FILE_PAGE_SIZE }, (_, index) =>
      indexedFile(`docs/readme-${index.toString().padStart(3, "0")}.md`),
    );
    mocks.oracleFiles.mockImplementation(async (_tab: string, page: number) =>
      page < 5 ? firstFourPages : [indexedFile("src/components/Component200.tsx")],
    );

    const result = await loadRepositoryLayers();

    expect(result.layers).toEqual([
      expect.objectContaining({
        id: "src/components/Component200.tsx",
        name: "Component200",
        kind: "TSX",
        source: { path: "src/components/Component200.tsx" },
      }),
    ]);
    expect(mocks.oracleFiles).toHaveBeenCalledWith("indexed", 5);
  });

  it("stops after a short page", async () => {
    mocks.oracleFiles.mockResolvedValue([indexedFile("src/Only.tsx"), indexedFile("src/Logo.svg")]);

    const result = await loadRepositoryLayers();

    expect(result.layers.map((layer) => [layer.name, layer.kind])).toEqual([
      ["Only", "TSX"],
      ["Logo", "SVG"],
    ]);
    expect(mocks.oracleFiles).toHaveBeenCalledTimes(1);
  });

  it("stops at the page cap and reports a partial list", async () => {
    mocks.oracleFiles.mockResolvedValue(
      Array.from({ length: ORACLE_FILE_PAGE_SIZE }, (_, index) =>
        indexedFile(`vendor/file-${index}.json`),
      ),
    );

    const result = await loadRepositoryLayers();

    expect(mocks.oracleFiles).toHaveBeenCalledTimes(MAX_ORACLE_FILE_PAGES);
    expect(result.layers).toEqual([]);
    expect(result.notice).toContain("partial");
  });

  it("filters extensions, maps kinds, and uses stable path ids", async () => {
    mocks.oracleFiles.mockResolvedValue([
      indexedFile("src/components/Header.tsx"),
      indexedFile("src/components/Header.test.tsx"),
      indexedFile("src/components/Header.spec.tsx"),
      indexedFile("assets/logo.svg"),
      indexedFile("src/components/fixtures/Fixture.svg"),
      indexedFile("crates/oracle-core/golden/corpus/src/components/App.tsx"),
      indexedFile("vendor/ThirdParty.svg"),
      indexedFile("src/components/__mocks__/Mock.tsx"),
      indexedFile("src/styles.css"),
      indexedFile("README.md"),
    ]);

    const first = await loadRepositoryLayers();
    const second = await loadRepositoryLayers();

    expect(first.layers).toEqual([
      expect.objectContaining({
        id: "src/components/Header.tsx",
        name: "Header",
        kind: "TSX",
        source: { path: "src/components/Header.tsx" },
      }),
      expect.objectContaining({
        id: "assets/logo.svg",
        name: "logo",
        kind: "SVG",
        source: { path: "assets/logo.svg" },
      }),
    ]);
    expect(second.layers.map((layer) => layer.id)).toEqual(first.layers.map((layer) => layer.id));
    expect(first.layers.every((layer) => layer.transform.x >= 0 && layer.transform.y >= 0)).toBe(
      true,
    );
  });

  it("stops requesting pages after an abort between page responses", async () => {
    const controller = new AbortController();
    let resolveFirstPage: ((files: readonly IndexedFile[]) => void) | undefined;
    const firstPage = new Promise<readonly IndexedFile[]>((resolve) => {
      resolveFirstPage = resolve;
    });
    const loadPage = vi
      .fn<(tab: FileTab, page: number) => Promise<readonly IndexedFile[]>>()
      .mockReturnValueOnce(firstPage)
      .mockResolvedValue([]);

    const result = loadRepositoryLayers(loadPage, controller.signal);
    controller.abort();
    resolveFirstPage?.([indexedFile("docs/readme.md")]);

    await expect(result).rejects.toMatchObject({ name: "AbortError" });
    expect(loadPage).toHaveBeenCalledTimes(1);
  });

  it("caps visible layers and says the component list is partial", async () => {
    mocks.oracleFiles.mockResolvedValue(
      Array.from({ length: ORACLE_FILE_PAGE_SIZE }, (_, index) =>
        indexedFile(`src/components/Component${index}.tsx`),
      ),
    );

    const result = await loadRepositoryLayers();

    expect(result.layers).toHaveLength(MAX_DESIGN_LAYERS);
    expect(result.notice).toContain("partial");
  });

  it("keeps paging when a full page reaches exactly the layer cap", async () => {
    const exactPage = [
      ...Array.from({ length: MAX_DESIGN_LAYERS }, (_, index) =>
        indexedFile(`src/components/Component${index}.tsx`),
      ),
      ...Array.from({ length: ORACLE_FILE_PAGE_SIZE - MAX_DESIGN_LAYERS }, (_, index) =>
        indexedFile(`docs/readme-${index}.md`),
      ),
    ];
    mocks.oracleFiles.mockResolvedValueOnce(exactPage).mockResolvedValueOnce([]);

    const result = await loadRepositoryLayers();

    expect(mocks.oracleFiles).toHaveBeenCalledTimes(2);
    expect(result.layers).toHaveLength(MAX_DESIGN_LAYERS);
    expect(result.notice).toBeUndefined();
  });

  it("continues after an oversized page response", async () => {
    mocks.oracleFiles
      .mockResolvedValueOnce(
        Array.from({ length: ORACLE_FILE_PAGE_SIZE + 1 }, (_, index) =>
          indexedFile(`docs/readme-${index}.md`),
        ),
      )
      .mockResolvedValueOnce([]);

    const result = await loadRepositoryLayers();

    expect(mocks.oracleFiles).toHaveBeenCalledTimes(2);
    expect(result.layers).toEqual([]);
    expect(result.notice).toContain("No TSX or SVG components");
  });

  it("returns no fixture layers when file enumeration fails", async () => {
    const failingLoader = vi.fn(async () => {
      throw new Error("No workspace is available.");
    });

    const result = await loadRepositoryLayers(failingLoader);

    expect(result.layers).toEqual([]);
    expect(result.layers).not.toContainEqual(expect.objectContaining({ id: "index-header" }));
    expect(result.notice).toContain("could not enumerate");
  });
});
