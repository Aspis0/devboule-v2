import { describe, expect, it } from "vitest";
import { treeKeyTarget, type TreeNode } from "./agentTreeKeys";

// root, its child, a second root.
const TREE: TreeNode[] = [
  { id: "root", parentId: null, hasChildren: true },
  { id: "child", parentId: "root", hasChildren: false },
  { id: "other", parentId: null, hasChildren: false },
];

describe("moving focus through the agent tree", () => {
  it("moves down and up one row, and stops at the ends", () => {
    expect(treeKeyTarget("ArrowDown", 0, TREE)).toBe(1);
    expect(treeKeyTarget("ArrowDown", 2, TREE)).toBeNull();
    expect(treeKeyTarget("ArrowUp", 1, TREE)).toBe(0);
    expect(treeKeyTarget("ArrowUp", 0, TREE)).toBeNull();
  });

  it("jumps to the first and the last row", () => {
    expect(treeKeyTarget("Home", 2, TREE)).toBe(0);
    expect(treeKeyTarget("End", 0, TREE)).toBe(2);
    expect(treeKeyTarget("Home", 0, TREE)).toBeNull();
  });

  it("enters a row's first child on the right, and goes nowhere on a row with none", () => {
    expect(treeKeyTarget("ArrowRight", 0, TREE)).toBe(1);
    expect(treeKeyTarget("ArrowRight", 2, TREE)).toBeNull();
  });

  it("returns from a child to its parent on the left, and goes nowhere from a root", () => {
    expect(treeKeyTarget("ArrowLeft", 1, TREE)).toBe(0);
    expect(treeKeyTarget("ArrowLeft", 0, TREE)).toBeNull();
  });

  it("leaves every other key to its own handler", () => {
    expect(treeKeyTarget("a", 0, TREE)).toBeNull();
    expect(treeKeyTarget("Enter", 0, TREE)).toBeNull();
  });
});
