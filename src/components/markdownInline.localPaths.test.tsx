// @vitest-environment happy-dom

// A markdown link to a local file opens it inside the workspace, and never shows its raw `[x](…)`.

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ChatFileLinks } from "../lib/chatFilePaths";
import { inline } from "./markdownInline";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const ROOT = "C:/Users/gualt/Desktop/test";

let root: Root | null = null;
let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement("div");
  document.body.append(host);
  root = createRoot(host);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  root = null;
  host.remove();
});

async function show(text: string, links: ChatFileLinks | null): Promise<void> {
  await act(async () => root?.render(<p>{inline(text, links)}</p>));
}

describe("markdown links to local files", () => {
  it("draws a file inside the workspace as a button that opens its relative path", async () => {
    const open = vi.fn();
    await show("[index.html](C:/Users/gualt/Desktop/test/kalsa-site/index.html)", {
      root: ROOT,
      open,
    });

    const link = host.querySelector<HTMLButtonElement>("button.plan-markdown-file-link");
    expect(link?.textContent).toBe("index.html");
    expect(host.textContent).not.toContain("](");
    await act(async () => link?.click());
    expect(open).toHaveBeenCalledWith("kalsa-site/index.html");
  });

  it("keeps a file outside the workspace as its label, never as a link to open", async () => {
    await show("[hosts](C:/Windows/System32/drivers/etc/hosts)", {
      root: ROOT,
      open: vi.fn(),
    });

    expect(host.querySelector("button")).toBeNull();
    expect(host.textContent).toBe("hosts");
    expect(host.querySelector("span[title='Outside this workspace']")).not.toBeNull();
  });

  it("keeps a local file as its label when no workspace is known", async () => {
    await show("[notes](~/notes/todo.md)", null);

    expect(host.querySelector("button")).toBeNull();
    expect(host.textContent).toBe("notes");
  });

  it("leaves a destination that is neither a web address nor a path as the text the agent wrote", async () => {
    await show("[run](javascript:alert(1))", { root: ROOT, open: vi.fn() });

    expect(host.textContent).toBe("[run](javascript:alert(1))");
  });
});
