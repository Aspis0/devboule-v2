// @vitest-environment happy-dom

// A path written as plain text or as code opens only when it is inside the workspace,
// by the same boundary a markdown link uses.

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

async function show(text: string, open = vi.fn()): Promise<ReturnType<typeof vi.fn>> {
  const links: ChatFileLinks = { root: ROOT, open };
  await act(async () => root?.render(<p>{inline(text, links)}</p>));
  return open;
}

describe("a path written as plain text", () => {
  it.each([
    ["an absolute path on another drive folder", "see C:/Windows/System32/secret.txt now"],
    [
      "a sibling folder that shares the root's name",
      "see C:/Users/gualt/Desktop/test2/notes.md now",
    ],
    ["a path that climbs out of the root", "see C:/Users/gualt/Desktop/test/../notes.md now"],
    ["a home path", "see ~/AppData/secret.txt now"],
  ])("draws no button for %s", async (_, text) => {
    const open = await show(text);

    expect(host.querySelector("button")).toBeNull();
    expect(open).not.toHaveBeenCalled();
    expect(host.textContent).toBe(text);
  });

  it("links a path inside the root, whatever separators it was written with", async () => {
    const open = await show("see C:\\Users\\gualt\\Desktop\\test\\kalsa\\index.html now");

    await act(async () => host.querySelector<HTMLButtonElement>("button")?.click());
    expect(open).toHaveBeenCalledWith("kalsa/index.html");
  });

  it("links a relative path, as before", async () => {
    const open = await show("see src/app.ts now");

    await act(async () => host.querySelector<HTMLButtonElement>("button")?.click());
    expect(open).toHaveBeenCalledWith("src/app.ts");
  });
});

describe("a path written as code", () => {
  it.each([
    ["an absolute path outside the root", "`C:/Windows/System32/secret.txt`"],
    ["a sibling folder that shares the root's name", "`C:/Users/gualt/Desktop/test2/notes.md`"],
    ["a home path", "`~/AppData/secret.txt`"],
  ])("draws no button for %s", async (_, text) => {
    const open = await show(text);

    expect(host.querySelector("button")).toBeNull();
    expect(open).not.toHaveBeenCalled();
  });

  it("links a code path inside the root", async () => {
    const open = await show("`src/app.ts`");

    await act(async () => host.querySelector<HTMLButtonElement>("button")?.click());
    expect(open).toHaveBeenCalledWith("src/app.ts");
  });
});
