import { oracleFiles } from "../../lib/tauri";
import type { FileTab, IndexedFile } from "../../types/ipc";
import type { DesignLayer, DesignLayerKind } from "./designHost";

export const ORACLE_FILE_PAGE_SIZE = 50;
// Each page currently re-walks the whole workspace on the daemon side. This cap therefore
// bounds the cost at twenty full walks, not merely twenty small slices of one listing.
export const MAX_ORACLE_FILE_PAGES = 20;
export const MAX_DESIGN_LAYERS = 36;

const GRID_COLUMNS = 4;
const GRID_ORIGIN_X = 60;
const GRID_ORIGIN_Y = 46;
const GRID_COLUMN_STEP = 312;
const GRID_ROW_STEP = 172;
const GRID_WIDTH = 280;
const GRID_HEIGHT = 140;
const TEST_OR_SPEC_FILE_PATTERN = /\.(?:test|spec)\.(?:tsx|ts|jsx|js)$/i;
const NON_SOURCE_DIRECTORY_SEGMENTS = new Set([
  "golden",
  "corpus",
  "fixtures",
  "__fixtures__",
  "__mocks__",
  "vendor",
  "node_modules",
  "dist",
  "build",
]);

export interface RepositoryLayerResult {
  layers: readonly DesignLayer[];
  notice?: string;
}

type FilesPageLoader = (tab: FileTab, page: number) => Promise<readonly IndexedFile[]>;

function throwIfAborted(signal: AbortSignal | undefined): void {
  if (signal?.aborted) {
    throw new DOMException("Repository layer enumeration aborted", "AbortError");
  }
}

function kindForPath(path: string): DesignLayerKind | undefined {
  if (path.toLowerCase().endsWith(".tsx")) return "TSX";
  if (path.toLowerCase().endsWith(".svg")) return "SVG";
  return undefined;
}

function nameForPath(path: string): string {
  const basename = path.slice(path.lastIndexOf("/") + 1);
  return basename.replace(/\.(tsx|svg)$/i, "");
}

function isDesignSourcePath(path: string): boolean {
  // The index supplies paths, not source roles. These explicit test/spec and conventional
  // directory filters are a deliberately small heuristic and can exclude a real component.
  if (TEST_OR_SPEC_FILE_PATTERN.test(path)) return false;
  return !path
    .toLowerCase()
    .split("/")
    .some((segment) => NON_SOURCE_DIRECTORY_SEGMENTS.has(segment));
}

function toLayer(file: IndexedFile, index: number, kind: DesignLayerKind): DesignLayer {
  // The repository has no geometry. This deterministic grid is a presentation choice here,
  // not a stored property of the source file.
  return {
    id: file.path,
    name: nameForPath(file.path),
    kind,
    transform: {
      x: GRID_ORIGIN_X + (index % GRID_COLUMNS) * GRID_COLUMN_STEP,
      y: GRID_ORIGIN_Y + Math.floor(index / GRID_COLUMNS) * GRID_ROW_STEP,
      width: GRID_WIDTH,
      height: GRID_HEIGHT,
    },
    source: { path: file.path },
  };
}

function componentFilesFrom(
  files: readonly IndexedFile[],
): Array<{ file: IndexedFile; kind: DesignLayerKind }> {
  return files.flatMap((file) => {
    if (!isDesignSourcePath(file.path)) return [];
    const kind = kindForPath(file.path);
    return kind === undefined ? [] : [{ file, kind }];
  });
}

function makeLayers(
  componentFiles: readonly { file: IndexedFile; kind: DesignLayerKind }[],
): DesignLayer[] {
  return componentFiles.map(({ file, kind }, index) => toLayer(file, index, kind));
}

export async function loadRepositoryLayers(
  loadPage: FilesPageLoader = oracleFiles,
  signal?: AbortSignal,
): Promise<RepositoryLayerResult> {
  // oracle_files has no cancellation hook, so cancellation is observed between page requests.
  const componentFiles: Array<{ file: IndexedFile; kind: DesignLayerKind }> = [];

  for (let page = 1; page <= MAX_ORACLE_FILE_PAGES; page += 1) {
    throwIfAborted(signal);
    let files: readonly IndexedFile[];
    try {
      files = await loadPage("indexed", page);
    } catch (error: unknown) {
      const detail = error instanceof Error && error.message ? `: ${error.message}` : "";
      return {
        layers: [],
        notice: `Oracle could not enumerate files for this workspace${detail}.`,
      };
    }
    throwIfAborted(signal);

    componentFiles.push(...componentFilesFrom(files));

    // A full or oversized page may have more files. Only call the result partial when
    // the layer cap actually truncates the list.
    if (componentFiles.length > MAX_DESIGN_LAYERS) {
      return {
        layers: componentFiles
          .slice(0, MAX_DESIGN_LAYERS)
          .map(({ file, kind }, index) => toLayer(file, index, kind)),
        notice: `Oracle's component list is partial; showing the first ${MAX_DESIGN_LAYERS} layers.`,
      };
    }

    if (files.length < ORACLE_FILE_PAGE_SIZE) {
      return {
        layers: makeLayers(componentFiles),
        notice:
          componentFiles.length === 0
            ? "No TSX or SVG components found in the indexed workspace."
            : undefined,
      };
    }
  }

  return {
    layers: makeLayers(componentFiles),
    notice:
      "Oracle's indexed file list is partial; component enumeration stopped at the safety limit.",
  };
}
