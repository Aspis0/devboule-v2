import { describe, expect, it, vi } from "vitest";
import type { WorkspaceDirectory, WorkspaceFileEntry } from "../../types/ipc";
import {
  FILE_MENTION_LIMIT,
  fileMentionQuery,
  insertFileMention,
  rankFileMatches,
} from "./fileMention";
import {
  FILE_INDEX_MAX_DIRECTORIES,
  FILE_INDEX_TTL_MS,
  indexWorkspaceFiles,
  resetWorkspaceFileIndexesForTests,
  workspaceFileIndex,
  type ListDirectory,
} from "./workspaceFileIndex";

describe("fileMentionQuery", () => {
  it("reads the tag being typed at the end of the draft", () => {
    expect(fileMentionQuery("@")).toBe("");
    expect(fileMentionQuery("@scout")).toBe("scout");
    expect(fileMentionQuery("look at @src/App")).toBe("src/App");
    expect(fileMentionQuery("look at\n@src")).toBe("src");
  });

  it("is null once the tag is closed or the draft has no tag", () => {
    expect(fileMentionQuery("look at @src/App.tsx ")).toBeNull();
    expect(fileMentionQuery("plain words")).toBeNull();
    expect(fileMentionQuery("mail me at name@host")).toBeNull();
    expect(fileMentionQuery("@a@b")).toBeNull();
  });
});

describe("rankFileMatches", () => {
  const paths = ["docs/plan.md", "src/App.tsx", "src/app/index.ts", "README.md"];

  it("ranks a file whose name starts with the query first", () => {
    expect(rankFileMatches(paths, "app")).toEqual(["src/App.tsx", "src/app/index.ts"]);
  });

  it("keeps any file whose path contains the query, and drops the rest", () => {
    expect(rankFileMatches(paths, "docs")).toEqual(["docs/plan.md"]);
    expect(rankFileMatches(paths, "zzz")).toEqual([]);
  });

  it("offers the first files when the query is empty, capped", () => {
    const many = Array.from({ length: FILE_MENTION_LIMIT + 4 }, (_, index) => `f${index}.ts`);
    expect(rankFileMatches(many, "")).toHaveLength(FILE_MENTION_LIMIT);
    expect(rankFileMatches(many, "")[0]).toBe("f0.ts");
  });
});

describe("insertFileMention", () => {
  it("replaces the open tag with the picked path and a space", () => {
    expect(insertFileMention("look at @sr", "src/App.tsx")).toBe("look at @src/App.tsx ");
    expect(insertFileMention("@", "README.md")).toBe("@README.md ");
  });
});

const reply = (
  path: string,
  entries: WorkspaceFileEntry[],
  error: string | null = null,
): WorkspaceDirectory => ({
  path,
  entries,
  capped: false,
  skipped: 0,
  error,
});

/** A small tree served one folder at a time, the way the daemon lists it. */
function treeLister(tree: Record<string, WorkspaceFileEntry[]>): {
  list: ListDirectory;
  calls: string[];
} {
  const calls: string[] = [];
  const list: ListDirectory = async (path) => {
    calls.push(path);
    const entries = tree[path];
    if (entries === undefined) {
      return reply(path, [], "not a folder");
    }
    return reply(path, entries);
  };
  return { list, calls };
}

const dir = (path: string): WorkspaceFileEntry => ({
  path,
  name: path.split("/").pop() ?? path,
  kind: "dir",
  size: null,
});
const file = (path: string): WorkspaceFileEntry => ({
  path,
  name: path.split("/").pop() ?? path,
  kind: "file",
  size: 1,
});

describe("indexWorkspaceFiles", () => {
  it("lists files breadth-first, skipping tool and build folders", async () => {
    const { list, calls } = treeLister({
      "": [dir("src"), dir("node_modules"), file("README.md")],
      src: [file("src/App.tsx")],
      node_modules: [file("node_modules/left-pad/index.js")],
    });
    expect((await indexWorkspaceFiles(list)).files).toEqual(["README.md", "src/App.tsx"]);
    expect(calls).toEqual(["", "src"]);
  });

  it("skips a folder the daemon refuses and one whose listing throws", async () => {
    const list: ListDirectory = async (path) => {
      if (path === "locked") throw new Error("denied");
      if (path === "broken") {
        return reply(path, [], "refused");
      }
      return reply(path, [dir("locked"), dir("broken"), file("ok.ts")]);
    };
    expect((await indexWorkspaceFiles(list)).files).toEqual(["ok.ts"]);
  });

  it("stops at the directory cap instead of walking the whole tree", async () => {
    let listed = 0;
    const endless: ListDirectory = async (path) => {
      listed += 1;
      return reply(path, [dir(`${path}d`)]);
    };
    const index = await indexWorkspaceFiles(endless);
    expect(listed).toBe(FILE_INDEX_MAX_DIRECTORIES);
    expect(index.truncated, "a walk the cap stopped says so").toBe(true);
  });
});

describe("workspaceFileIndex", () => {
  it("builds one index per workspace and reuses it", async () => {
    resetWorkspaceFileIndexesForTests();
    const { list, calls } = treeLister({ "": [file("a.ts")] });
    const first = await workspaceFileIndex("w1", list);
    const second = await workspaceFileIndex("w1", list);
    expect(first.files).toEqual(["a.ts"]);
    expect(second).toBe(first);
    expect(calls).toEqual([""]);
  });

  it("rebuilds after its minute, so a file created since is found", async () => {
    vi.useFakeTimers();
    resetWorkspaceFileIndexesForTests();
    let tree: Record<string, WorkspaceFileEntry[]> = { "": [file("a.ts")] };
    const live: ListDirectory = (path) => treeLister(tree).list(path);
    expect((await workspaceFileIndex("w3", live)).files).toEqual(["a.ts"]);
    tree = { "": [file("a.ts"), file("b.ts")] };
    expect((await workspaceFileIndex("w3", live)).files).toEqual(["a.ts"]);
    await vi.advanceTimersByTimeAsync(FILE_INDEX_TTL_MS);
    expect((await workspaceFileIndex("w3", live)).files).toEqual(["a.ts", "b.ts"]);
    vi.useRealTimers();
  });
});
