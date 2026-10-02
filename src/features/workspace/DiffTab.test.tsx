// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { DiffTab, ROW_CLASS, resetDiffTabModeMemoryForTests } from "./DiffTab";
import type { WorkspaceGitDiffLine, WorkspaceGitFileDiff } from "../../types/ipc";
import type { ChangesReply } from "./useWorkspaceChanges";

vi.mock("../../lib/tauri", () => ({
  workspaceFileOpen: vi.fn(),
  editorTargetsList: vi.fn(),
}));

import { editorTargetsList, workspaceFileOpen } from "../../lib/tauri";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const WORKSPACE = "workspace-diff-tab-subject";

function okReply(
  lines: WorkspaceGitDiffLine[],
  overrides: Partial<WorkspaceGitFileDiff> = {},
): WorkspaceGitFileDiff {
  return {
    path: "src/checkout.ts",
    isNew: false,
    isDeleted: false,
    additions: 15,
    deletions: 6,
    lines,
    status: "ok",
    error: null,
    ...overrides,
  };
}

function refusalReply(status: "binary" | "too_large" | "error", error: string | null) {
  return okReply([], { additions: 0, deletions: 0, lines: [], status, error });
}

// Accessible text: subtrees the author hid from assistive tech do not count.
function accessibleText(root: Element): string {
  if (root.getAttribute("aria-hidden") === "true") return "";
  let out = "";
  for (const child of [...root.childNodes]) {
    if (child.nodeType === 3) out += child.textContent ?? "";
    else if (child.nodeType === 1) out += accessibleText(child as Element);
  }
  return out;
}

const LINES: WorkspaceGitDiffLine[] = [
  { kind: "header", text: "@@ -12,3 +12,4 @@ import" },
  { kind: "context", text: 'import { detectGates } from "./gates";' },
  { kind: "remove", text: 'import { legacyFallback } from "./legacy";' },
  { kind: "add", text: 'import { providers } from "./registry";' },
  { kind: "add", text: 'import { resolveProvider } from "./resolve";' },
];

describe("DiffTab", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    resetDiffTabModeMemoryForTests();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    vi.mocked(workspaceFileOpen).mockResolvedValue(undefined);
    vi.mocked(editorTargetsList).mockResolvedValue([
      { id: "cursor", label: "Cursor", kind: "editor" },
    ]);
    localStorage.clear();
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(path: string, diff: ChangesReply<WorkspaceGitFileDiff>) {
    await act(async () => {
      root.render(<DiffTab workspaceId={WORKSPACE} path={path} diff={diff} />);
    });
  }

  function segButton(label: "Unified" | "Split"): HTMLButtonElement {
    const button = [...container.querySelectorAll(".diff-tab-seg-button")].find(
      (candidate) => candidate.textContent === label,
    );
    if (!(button instanceof HTMLButtonElement)) throw new Error(`${label} button did not render`);
    return button;
  }

  it("names the file, its directory, and the stats", async () => {
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    expect(container.querySelector(".diff-tab-name")?.textContent).toBe("checkout.ts");
    expect(container.querySelector(".diff-tab-dir")?.textContent).toBe("src");
    expect(container.querySelector(".diff-tab-stats-add")?.textContent).toBe("+15");
    expect(container.querySelector(".diff-tab-stats-del")?.textContent).toBe("−6");
  });

  it("offers Open in editor with the pane's workspace id and path", async () => {
    // An empty reply has no hunk header, so the file opens without a line.
    await render("src/checkout.ts", { reply: okReply([]), failure: null });
    const button = container.querySelector<HTMLButtonElement>(".open-in-editor-button");
    if (button === null) throw new Error("the pencil action did not render");
    expect(button.getAttribute("aria-label")).toBe("Open in editor");
    expect(button.getAttribute("title")).toBe("Open in editor");

    await act(async () => {
      button.click();
    });

    expect(workspaceFileOpen).toHaveBeenCalledWith(
      WORKSPACE,
      "src/checkout.ts",
      undefined,
      "cursor",
    );
  });

  it("passes the first hunk's new-side line to the editor", async () => {
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    const button = container.querySelector<HTMLButtonElement>(".open-in-editor-button");
    if (button === null) throw new Error("the pencil action did not render");

    await act(async () => {
      button.click();
    });

    // The first header is "@@ -12,3 +12,4 @@ import": the new side starts at 12.
    expect(workspaceFileOpen).toHaveBeenCalledWith(WORKSPACE, "src/checkout.ts", 12, "cursor");
  });

  it("starts unified, and the toggle switches the layout", async () => {
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    expect(segButton("Unified").getAttribute("aria-pressed")).toBe("true");
    expect(container.querySelectorAll(".diff-tab-line")).toHaveLength(4);
    expect(container.querySelectorAll(".diff-tab-hunk")).toHaveLength(1);

    await act(async () => {
      segButton("Split").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(segButton("Split").getAttribute("aria-pressed")).toBe("true");
    expect(container.querySelectorAll(".diff-tab-line")).toHaveLength(0);
    expect(container.querySelectorAll(".diff-tab-split-row")).toHaveLength(3);
  });

  it("remembers the mode when another Diff tab mounts", async () => {
    await render("src/a.ts", { reply: okReply(LINES), failure: null });
    await act(async () => {
      segButton("Split").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    // Another tab mounts fresh — a new path, a new DiffTab — and inherits it.
    await render("src/b.ts", { reply: okReply(LINES), failure: null });
    expect(segButton("Split").getAttribute("aria-pressed")).toBe("true");
    expect(container.querySelectorAll(".diff-tab-split-row")).toHaveLength(3);
  });

  it("numbers unified rows old then new, blanking the missing side", async () => {
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    const numbers = [...container.querySelectorAll(".diff-tab-line")].map((row) =>
      [...row.querySelectorAll(".diff-tab-num")].map((cell) => cell.textContent),
    );
    expect(numbers).toEqual([
      ["12", "12"],
      ["13", ""],
      ["", "13"],
      ["", "14"],
    ]);
  });

  it("pairs split rows with an empty cell for the missing side", async () => {
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    await act(async () => {
      segButton("Split").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const rows = [...container.querySelectorAll(".diff-tab-split-row")];
    expect(rows).toHaveLength(3);
    const texts = rows.map((row) => [
      row.children[0]?.textContent ?? null,
      row.children[1]?.textContent ?? null,
    ]);
    expect(texts[0]![0]).toContain("./gates");
    expect(texts[0]![1]).toContain("./gates");
    expect(texts[1]![0]).toContain("./legacy");
    expect(texts[1]![1]).toContain("./registry");
    expect(rows[2]!.children[0]!.className).toContain("diff-tab-cell-empty");
    expect(texts[2]![1]).toContain("./resolve");
  });

  it.each([
    ["binary", "binary", "This file is binary; there are no lines to show."],
    ["too_large", "too large", "Diff over the 16 KiB cap."],
    ["error", "error", "Git refused the diff."],
  ] as const)("shows the %s state with the header still up", async (status, word, error) => {
    await render("src/checkout.ts", { reply: refusalReply(status, error), failure: null });
    expect(container.querySelector(".diff-tab-name")?.textContent).toBe("checkout.ts");
    expect(container.querySelector(".diff-tab-stats-word")?.textContent).toBe(word);
    expect(container.querySelector(".diff-tab-body")?.textContent).toContain(error);
    expect(container.querySelectorAll(".diff-tab-line")).toHaveLength(0);
  });

  it("names an empty ok reply instead of showing rows", async () => {
    await render("src/checkout.ts", { reply: okReply([]), failure: null });
    expect(container.querySelector(".diff-tab-body")?.textContent).toContain(
      "This file has no uncommitted line changes.",
    );
  });

  it("shows loading until the first read lands", async () => {
    await render("src/checkout.ts", { reply: null, failure: null });
    expect(container.querySelector('.diff-tab-body [role="status"]')?.textContent).toContain(
      "Loading diff",
    );
  });

  it("shows a refused read beside the header", async () => {
    await render("src/checkout.ts", {
      reply: null,
      failure: { sentence: "Git exploded.", detail: null },
    });
    expect(container.querySelector('.diff-tab-body [role="alert"]')?.textContent).toContain(
      "Git exploded.",
    );
  });

  it("marks added and removed rows with the shared class names", async () => {
    // The map is the join: the component renders from it, the stylesheet
    // defines these selectors, so a rename on either side breaks here.
    expect(ROW_CLASS).toEqual({
      add: "diff-tab-added",
      remove: "diff-tab-removed",
      context: "diff-tab-context",
      header: "diff-tab-hunk",
    });
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    const rows = [...container.querySelectorAll(".diff-tab-line")];
    expect(rows).toHaveLength(4);
    expect(rows[1]!.className).toContain("diff-tab-removed");
    expect(rows[2]!.className).toContain("diff-tab-added");
    expect(rows[3]!.className).toContain("diff-tab-added");
  });

  it("gives every row a non-colour marker, hidden words included", async () => {
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    const markers = [...container.querySelectorAll(".diff-tab-line .diff-tab-marker")].map(
      (marker) => ({
        glyph: marker.textContent,
        hidden: marker.getAttribute("aria-hidden"),
      }),
    );
    expect(markers).toEqual([
      { glyph: "\u00A0", hidden: "true" },
      { glyph: "\u2212", hidden: "true" },
      { glyph: "+", hidden: "true" },
      { glyph: "+", hidden: "true" },
    ]);
    const words = [...container.querySelectorAll(".diff-tab-line .diff-tab-visually-hidden")].map(
      (word) => word.textContent,
    );
    expect(words).toEqual(["removed", "added", "added"]);

    await act(async () => {
      segButton("Split").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const splitMarkers = [
      ...container.querySelectorAll(".diff-tab-split-row .diff-tab-marker"),
    ].map((marker) => marker.textContent);
    expect(splitMarkers).toEqual(["\u00A0", "\u00A0", "\u2212", "+", "+"]);
    const splitWords = [
      ...container.querySelectorAll(".diff-tab-split-row .diff-tab-visually-hidden"),
    ].map((word) => word.textContent);
    expect(splitWords).toEqual(["removed", "added", "added"]);
  });

  it("keeps the diff on screen beside a refresh failure", async () => {
    await render("src/checkout.ts", {
      reply: okReply(LINES),
      failure: { sentence: "The daemon went away.", detail: null },
    });
    expect(container.querySelector(".diff-tab-body")?.textContent).toContain("./registry");
    expect(container.querySelector(".diff-tab-refresh-failure")?.textContent).toContain(
      "Couldn't refresh: The daemon went away.",
    );
  });

  it("renders a combined diff's rows with blank numbers", async () => {
    const combined: WorkspaceGitDiffLine[] = [
      { kind: "header", text: "@@@ -1,2 -1,2 +1,3 @@@" },
      { kind: "remove", text: "old" },
      { kind: "add", text: "new" },
    ];
    await render("src/checkout.ts", { reply: okReply(combined), failure: null });
    const rows = [...container.querySelectorAll(".diff-tab-line")];
    expect(rows).toHaveLength(2);
    expect(rows[0]!.textContent).toContain("old");
    expect(rows[1]!.textContent).toContain("new");
    const numbers = rows.map((row) =>
      [...row.querySelectorAll(".diff-tab-num")].map((cell) => cell.textContent),
    );
    expect(numbers).toEqual([
      ["", ""],
      ["", ""],
    ]);
  });

  it("paints the numbers inside the tinted rows they are measured on", async () => {
    // The contrast suite measures --tone-idle-text on the row tints: if the
    // markup ever moved the numbers out of the rows, those grounds would lie.
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    const tinted = [
      ...container.querySelectorAll(
        ".diff-tab-line.diff-tab-added, .diff-tab-line.diff-tab-removed",
      ),
    ];
    expect(tinted.length).toBeGreaterThan(0);
    for (const row of tinted) {
      expect(row.querySelectorAll(".diff-tab-num")).toHaveLength(2);
    }

    await act(async () => {
      segButton("Split").dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    const cells = [
      ...container.querySelectorAll(
        ".diff-tab-cell.diff-tab-added, .diff-tab-cell.diff-tab-removed",
      ),
    ];
    expect(cells.length).toBeGreaterThan(0);
    for (const cell of cells) {
      expect(cell.querySelectorAll(".diff-tab-num")).toHaveLength(1);
    }
  });

  it("renders hunk rows from the shared class map, in both layouts", async () => {
    // A literal in the JSX would pass the map pin above and still answer a
    // rename with a dead rule; the probe proves the render follows the map.
    const real = ROW_CLASS.header;
    ROW_CLASS.header = "diff-tab-hunk-probe";
    try {
      await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
      expect(container.querySelectorAll(".diff-tab-hunk-probe")).toHaveLength(1);
      await act(async () => {
        segButton("Split").dispatchEvent(new MouseEvent("click", { bubbles: true }));
      });
      expect(container.querySelectorAll(".diff-tab-hunk-probe")).toHaveLength(1);
    } finally {
      ROW_CLASS.header = real;
    }
  });

  it("keeps one live region mounted, empty until a failure lands", async () => {
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    const before = container.querySelector('.diff-tab-header [role="status"]');
    expect(before).not.toBeNull();
    expect(before?.textContent).toBe("");
    await render("src/checkout.ts", {
      reply: okReply(LINES),
      failure: { sentence: "The daemon went away.", detail: null },
    });
    const after = container.querySelector('.diff-tab-header [role="status"]');
    expect(after).not.toBeNull();
    expect(after).toBe(before);
    expect(after?.textContent).toContain("The daemon went away.");
  });

  it("announces the refresh failure through a polite live region", async () => {
    await render("src/checkout.ts", {
      reply: okReply(LINES),
      failure: { sentence: "The daemon went away.", detail: null },
    });
    const live = container.querySelector('.diff-tab-header [role="status"]');
    expect(live?.textContent).toContain("The daemon went away.");
    expect(container.querySelector(".diff-tab-refresh-failure")?.textContent).toContain(
      "The daemon went away.",
    );
  });

  it("reads the failure sentence exactly once", async () => {
    await render("src/checkout.ts", {
      reply: okReply(LINES),
      failure: { sentence: "The daemon went away.", detail: null },
    });
    const header = container.querySelector(".diff-tab-header");
    if (header === null) throw new Error("header did not render");
    expect(container.querySelector(".diff-tab-refresh-failure")?.getAttribute("aria-hidden")).toBe(
      "true",
    );
    expect(accessibleText(header).split("The daemon went away.")).toHaveLength(2);
  });

  it("speaks a changed failure reason, and empties the region on recovery", async () => {
    const failure = (sentence: string) => ({
      reply: okReply(LINES),
      failure: { sentence, detail: null },
    });
    await render("src/checkout.ts", failure("The daemon went away."));
    expect(container.querySelector(".diff-tab-refresh-failure")?.textContent).toContain(
      "went away",
    );
    await render("src/checkout.ts", failure("The file is locked."));
    expect(container.querySelector(".diff-tab-refresh-failure")?.textContent).toContain("locked");
    // Clearing drops the visible line and empties the live shell: silence, not a second message.
    await render("src/checkout.ts", { reply: okReply(LINES), failure: null });
    expect(container.querySelector(".diff-tab-refresh-failure")).toBeNull();
    expect(container.querySelector('.diff-tab-header [role="status"]')?.textContent).toBe("");
  });
});
