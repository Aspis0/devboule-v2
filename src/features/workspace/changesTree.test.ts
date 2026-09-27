// @vitest-environment happy-dom

import { describe, expect, it } from "vitest";
import type { WorkspaceGitRow } from "../../types/ipc";
import { buildChangesTree, type ChangesTreeFolder } from "./changesTree";

function row(overrides: Partial<WorkspaceGitRow> & { path: string }): WorkspaceGitRow {
  return { additions: 0, deletions: 0, status: "modified", capped: false, ...overrides };
}

function folderOf(nodes: ReturnType<typeof buildChangesTree>, path: string): ChangesTreeFolder {
  const found = nodes.find((node) => node.kind === "folder" && node.path === path);
  if (found === undefined || found.kind !== "folder")
    throw new Error(`folder ${path} did not build`);
  return found;
}

describe("buildChangesTree", () => {
  it("keeps root files top-level in the daemon's order, never sorted", () => {
    const nodes = buildChangesTree([
      row({ path: "zebra.ts", additions: 1 }),
      row({ path: "apple.ts", additions: 2 }),
    ]);

    expect(nodes.map((node) => `${node.kind}:${node.path}`)).toEqual([
      "file:zebra.ts",
      "file:apple.ts",
    ]);
  });

  it("groups nested rows into folders with post-order sums", () => {
    const nodes = buildChangesTree([
      row({ path: "src/a.ts", additions: 2, deletions: 1 }),
      row({ path: "src/sub/b.ts", additions: 5, deletions: 0 }),
      row({ path: "docs/guide.md", additions: 9, deletions: 4 }),
    ]);

    const src = folderOf(nodes, "src");
    expect(src.additions).toBe(7);
    expect(src.deletions).toBe(1);
    expect(src.capped).toBe(false);
    const sub = folderOf(src.children, "src/sub");
    expect(sub.additions).toBe(5);
    expect(sub.deletions).toBe(0);
    const docs = folderOf(nodes, "docs");
    expect(docs.additions).toBe(9);
    expect(docs.deletions).toBe(4);
  });

  it("marks every ancestor of a capped row with the floor sums", () => {
    const nodes = buildChangesTree([
      row({ path: "src/exact.ts", additions: 3, deletions: 1 }),
      row({ path: "src/sub/floor.ts", additions: 40, deletions: 2, capped: true }),
    ]);

    const src = folderOf(nodes, "src");
    expect(src.additions).toBe(43);
    expect(src.capped).toBe(true);
    const sub = folderOf(src.children, "src/sub");
    expect(sub.capped).toBe(true);
    const exact = src.children.find((node) => node.kind === "file" && node.path === "src/exact.ts");
    if (exact === undefined || exact.kind !== "file") throw new Error("exact row did not build");
    expect(exact.row.capped).toBe(false);
  });

  it("marks every folder estimated when the reply carries a caveat", () => {
    const nodes = buildChangesTree(
      [row({ path: "src/exact.ts", additions: 3, deletions: 1 })],
      true,
    );

    const src = folderOf(nodes, "src");
    expect(src.additions).toBe(3);
    expect(src.capped).toBe(true);
  });

  it("groups a rename under its new path, acting on both sides", () => {
    const nodes = buildChangesTree([
      row({
        path: "docs/new.ts",
        renamedFrom: "src/old.ts",
        additions: 2,
        deletions: 5,
      }),
    ]);

    // The old side's deletion has no folder of its own: the acts take
    // both paths (pathsOf), the tree shows where the file stands now.
    expect(nodes.map((node) => `${node.kind}:${node.path}`)).toEqual(["folder:docs"]);
    const docs = folderOf(nodes, "docs");
    expect(docs.additions).toBe(2);
    expect(docs.deletions).toBe(5);
  });

  it("never splits a POSIX filename on its backslash", () => {
    // `git status --porcelain=v2 -z` prints `/` on every platform, so `/`
    // is the only separator this builder knows; a backslash inside a name
    // is data, not depth.
    const nodes = buildChangesTree([row({ path: "a\\b.ts", additions: 1 })]);
    expect(nodes.map((node) => `${node.kind}:${node.path}`)).toEqual(["file:a\\b.ts"]);
  });

  it("shows each file under its basename with the full path kept for acts", () => {
    const nodes = buildChangesTree([row({ path: "src/checkout/registry.ts", additions: 4 })]);

    const src = folderOf(nodes, "src");
    const checkout = folderOf(src.children, "src/checkout");
    const file = checkout.children.find((node) => node.kind === "file");
    if (file === undefined || file.kind !== "file") throw new Error("file row did not build");
    expect(file.name).toBe("registry.ts");
    expect(file.path).toBe("src/checkout/registry.ts");
    expect(file.row.additions).toBe(4);
  });

  it("returns no nodes for no rows", () => {
    expect(buildChangesTree([])).toEqual([]);
  });
});
