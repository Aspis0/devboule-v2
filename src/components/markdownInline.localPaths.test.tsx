// @vitest-environment happy-dom

// A markdown link to a local file opens it only when the file is inside the workspace.

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

function links(open = vi.fn()): ChatFileLinks {
  return { root: ROOT, open };
}

describe("a file inside the workspace", () => {
  it("draws a button that opens its root-relative path", async () => {
    const open = vi.fn();
    await show("[index.html](C:/Users/gualt/Desktop/test/kalsa-site/index.html)", links(open));

    const link = host.querySelector<HTMLButtonElement>("button.plan-markdown-file-link");
    expect(link?.textContent).toBe("index.html");
    expect(host.textContent).not.toContain("](");
    await act(async () => link?.click());
    expect(open).toHaveBeenCalledWith("kalsa-site/index.html");
  });
});

describe("a file outside the workspace", () => {
  it.each([
    [
      "an absolute path elsewhere on the drive",
      "[secrets](C:/Windows/System32/drivers/etc/secrets.txt)",
    ],
    [
      "a sibling folder that shares the root's name",
      "[notes](C:/Users/gualt/Desktop/test2/notes.md)",
    ],
    ["a path that climbs out of the root", "[notes](C:/Users/gualt/Desktop/test/../notes.md)"],
    ["a home path", "[secret](~/AppData/secret.txt)"],
    ["a file URL naming another machine", "[secret](file://server/share/secrets.txt)"],
  ])("shows the label only, with no link: %s", async (_, text) => {
    const open = vi.fn();
    await show(text, links(open));

    expect(host.querySelector("button")).toBeNull();
    expect(host.querySelector("a")).toBeNull();
    expect(host.textContent).not.toContain("](");
    expect(host.querySelector("span[title='Outside this workspace']")).not.toBeNull();
    expect(open).not.toHaveBeenCalled();
  });

  it("shows the label when no workspace is known", async () => {
    await show("[notes](~/notes/todo.md)", null);

    expect(host.querySelector("button")).toBeNull();
    expect(host.textContent).toBe("notes");
  });
});

describe("a destination that is not a path", () => {
  it("shows the label as plain text, never the raw markdown", async () => {
    await show("[run](javascript:alert(1))", links());

    expect(host.querySelector("a")).toBeNull();
    expect(host.textContent).toBe("run");
  });
});
