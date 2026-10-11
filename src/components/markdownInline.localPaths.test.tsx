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

describe("a folder", () => {
  it("shows a folder written with a trailing slash as plain text", async () => {
    const open = vi.fn();
    await show("[mockups](mockups/)", links(open));

    expect(host.querySelector("button")).toBeNull();
    expect(open).not.toHaveBeenCalled();
    expect(host.textContent).toBe("mockups");
  });

  it("keeps a link to a name without a trailing separator, and marks it Folder when the editor says so", async () => {
    const open = vi.fn().mockResolvedValue("folder");
    await show("[screenshots](mockups/screenshots)", links(open));

    const link = host.querySelector<HTMLButtonElement>("button.plan-markdown-file-link");
    expect(link?.title).toBe("mockups/screenshots");
    await act(async () => link?.click());

    expect(open).toHaveBeenCalledWith("mockups/screenshots");
    expect(link?.textContent).toBe("screenshots");
    expect(link?.title).toBe("Folder");
    expect(host.querySelector("[role='alert']")).toBeNull();
  });

  it.each([
    ["Gemfile", "[gemfile](Gemfile)"],
    ["README", "[readme](README)"],
    [".github", "[workflows](.github)"],
    ["assets.v2", "[assets](assets.v2)"],
  ])("links %s, which is a file or a folder the editor decides", async (_, text) => {
    const open = vi.fn().mockResolvedValue("file");
    await show(text, links(open));

    await act(async () => host.querySelector<HTMLButtonElement>("button")?.click());
    expect(open).toHaveBeenCalledTimes(1);
    expect(host.querySelector<HTMLButtonElement>("button")?.title).not.toBe("Folder");
  });

  it("leaves a folder written with a trailing backslash as the text the agent wrote", async () => {
    // The backslash escapes the closing parenthesis, so the parser reads no link at all.
    await show("[mockups](mockups\\)", links());

    expect(host.querySelector("button")).toBeNull();
    expect(host.textContent).toBe("[mockups](mockups)");
  });

  it("still links a file whose name has no extension but is a known file name", async () => {
    await show("[license](LICENSE)", links());

    expect(host.querySelector("button.plan-markdown-file-link")?.textContent).toBe("license");
  });
});

describe("a destination that is not a path", () => {
  it("shows the label as plain text, never the raw markdown", async () => {
    await show("[run](javascript:alert(1))", links());

    expect(host.querySelector("a")).toBeNull();
    expect(host.textContent).toBe("run");
  });
});
