// @vitest-environment happy-dom
import { act } from "react";
import { createRoot } from "react-dom/client";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ChatFileLinks } from "../lib/chatFilePaths";
import { parseMarkdownText } from "./markdownParser";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ROOT = "/home/u/repo";

function render(text: string, links?: ChatFileLinks | null): string {
  return renderToStaticMarkup(<div>{parseMarkdownText(text, links)}</div>);
}

function buttonMarkup(text: string): string {
  return `<button type="button" class="plan-markdown-file-link" title="${text}">${text}</button>`;
}

describe("file links in markdown", () => {
  it("renders a prose path as a button and keeps the trailing comma outside it", () => {
    expect(render("edit src/a.ts, then run", { root: ROOT, open: () => undefined })).toBe(
      `<div><p>edit ${buttonMarkup("src/a.ts")}, then run</p></div>`,
    );
  });

  it("links a code span whose displayed bytes equal its resolved target", () => {
    expect(render("touch `src/a.ts` now", { root: ROOT, open: () => undefined })).toBe(
      `<div><p>touch <code>${buttonMarkup("src/a.ts")}</code> now</p></div>`,
    );
  });

  it.each([
    "src/a\\.ts",
    "src/a\\_b.ts",
    "src/a\\#b.ts",
    "src/a.ts:0",
    "src/a.ts:1:0",
    "src/a.ts)",
    "./src/a.ts",
    "./src/a.ts:12",
    "src/x/../a.ts",
    "src/x/../a.ts:12",
    "src\\a.ts",
  ])("keeps code plain when displayed bytes differ from the resolved target: %s", (candidate) => {
    expect(render(`touch \`${candidate}\` now`, { root: ROOT, open: () => undefined })).toBe(
      `<div><p>touch <code>${candidate}</code> now</p></div>`,
    );
  });

  it.each(["src/a\tb.ts", "src/a\nb.ts", " src/a.ts", "src/a.ts "])(
    "keeps whitespace other than embedded spaces plain in code: %s",
    (candidate) => {
      expect(
        render(`touch \`${candidate}\` now`, { root: ROOT, open: () => undefined }),
      ).not.toContain("plan-markdown-file-link");
    },
  );

  it("keeps a spaced absolute prose path plain", () => {
    const candidate = String.raw`C:\Users\u\New folder\repo\src\a.ts`;
    expect(
      render(candidate, { root: String.raw`\\?\C:\Users\u\New folder\repo`, open: vi.fn() }),
    ).not.toContain("plan-markdown-file-link");
  });

  it("preserves multiline rendering while refusing a code path across lines", () => {
    const text = "touch `src/a\nb.ts` and **some\nwords** now";
    expect(render(text, { root: ROOT, open: vi.fn() })).toBe(render(text));
    expect(render(text)).toContain("<code>src/a b.ts</code>");
  });

  it("leaves fenced code, explicit links and URLs untouched", () => {
    const links = { root: ROOT, open: vi.fn() };
    expect(render("```\nsrc/a.ts\n```", links)).not.toContain("plan-markdown-file-link");
    expect(render("[a](src/x.ts)", links)).toContain("[a](src/x.ts)");
    expect(render("[docs](https://e.com)", links)).toContain(
      '<a href="https://e.com" target="_blank" rel="noreferrer">docs</a>',
    );
    expect(render("see https://x/y.ts now", links)).not.toContain("plan-markdown-file-link");
    expect(links.open).not.toHaveBeenCalled();
  });

  it("renders nothing different when the feature is off", () => {
    const fixture =
      "edit src/a.ts, then `src/b.ts` and [docs](https://e.com).\n\n- item src/c.ts:12\n";
    expect(render(fixture, null)).toBe(render(fixture));
    expect(render(fixture)).not.toContain("<button");
  });
});

describe("clicking a file link", () => {
  let container: HTMLDivElement;

  afterEach(async () => {
    container.remove();
  });

  it.each([
    { candidate: "src/x.ts:12", workspaceRoot: ROOT, relativePath: "src/x.ts" },
    { candidate: "src/x.ts:12:3", workspaceRoot: ROOT, relativePath: "src/x.ts" },
    { candidate: "src/New folder/a.ts", workspaceRoot: ROOT, relativePath: "src/New folder/a.ts" },
    { candidate: "/home/u/repo/src/a.ts", workspaceRoot: ROOT, relativePath: "src/a.ts" },
    {
      candidate: String.raw`C:\Users\u\New folder\repo\src\a.ts`,
      workspaceRoot: String.raw`\\?\C:\Users\u\New folder\repo`,
      relativePath: "src/a.ts",
    },
    {
      candidate: String.raw`\\?\C:\Users\u\New folder\repo\src\a.ts`,
      workspaceRoot: String.raw`C:\Users\u\New folder\repo`,
      relativePath: "src/a.ts",
    },
    {
      candidate: String.raw`\\?\C:\Users\u\New folder\repo\src\a.ts:12:3`,
      workspaceRoot: String.raw`\\?\C:\Users\u\New folder\repo`,
      relativePath: "src/a.ts",
    },
  ])(
    "preserves the code label and opens its relative target: $candidate",
    async ({ candidate, workspaceRoot, relativePath }) => {
      const open = vi.fn();
      container = document.createElement("div");
      document.body.appendChild(container);
      const root = createRoot(container);
      try {
        await act(async () => {
          root.render(
            <div>
              {parseMarkdownText(`touch \`${candidate}\` now`, { root: workspaceRoot, open })}
            </div>,
          );
        });
        const button = container.querySelector<HTMLButtonElement>(
          "code > button.plan-markdown-file-link",
        );
        if (button === null) throw new Error("code file link did not render");
        expect(button.textContent).toBe(candidate);
        expect(button.title).toBe(relativePath);
        await act(async () => button.click());
        expect(open).toHaveBeenCalledTimes(1);
        expect(open).toHaveBeenCalledWith(relativePath);
      } finally {
        await act(async () => root.unmount());
      }
    },
  );

  it("calls open once with the relative path", async () => {
    const open = vi.fn();
    container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(<div>{parseMarkdownText("edit src/a.ts, then run", { root: ROOT, open })}</div>);
    });
    const button = container.querySelector<HTMLButtonElement>("button.plan-markdown-file-link");
    if (button === null) throw new Error("file link did not render");
    expect(button.getAttribute("title")).toBe("src/a.ts");
    await act(async () => {
      button.click();
    });
    expect(open).toHaveBeenCalledTimes(1);
    expect(open).toHaveBeenCalledWith("src/a.ts");
    await act(async () => root.unmount());
  });

  it.each([
    { candidate: "/home/u/repo/src/a.ts", workspaceRoot: ROOT },
    { candidate: "C:\\repo\\src\\a.ts", workspaceRoot: "C:\\repo" },
  ])(
    "opens only the resolved relative path for prose $candidate",
    async ({ candidate, workspaceRoot }) => {
      const open = vi.fn();
      container = document.createElement("div");
      document.body.appendChild(container);
      const root = createRoot(container);
      try {
        await act(async () => {
          root.render(
            <div>{parseMarkdownText(`edit ${candidate} now`, { root: workspaceRoot, open })}</div>,
          );
        });
        const button = container.querySelector<HTMLButtonElement>("button.plan-markdown-file-link");
        if (button === null) throw new Error("file link did not render");
        expect(button.textContent).toBe(candidate);
        expect(button.title).toBe("src/a.ts");
        await act(async () => button.click());
        expect(open).toHaveBeenCalledTimes(1);
        expect(open).toHaveBeenCalledWith("src/a.ts");
      } finally {
        await act(async () => root.unmount());
      }
    },
  );
});
