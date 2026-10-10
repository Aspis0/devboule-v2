// The composer's "@" menu: the workspace files it offers, what Enter and Tab
// put into the text, the index it reads once per mention, and the states it
// shows while that index is still coming. The "/" menu is in
// `WorkspaceComposer.commandMenu.test.tsx`.
// @vitest-environment happy-dom
import { act, type ComponentProps } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { composerDrivers, composerProps, type ComposerDrivers } from "./composerTestKit";
import type { PromptAttachment } from "../../types/ipc";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement;
let root: Root;
let onSend: Mock<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>;
let onQueue: Mock<(text: string, attachments: readonly PromptAttachment[]) => void>;
let drive: ComposerDrivers;

async function renderComposer(
  overrides: Partial<ComponentProps<typeof WorkspaceComposer>> = {},
): Promise<void> {
  root = createRoot(container);
  await act(async () => {
    root.render(
      <WorkspaceComposer
        {...composerProps({ onSend, onQueue }, { availableCommands: [], ...overrides })}
      />,
    );
  });
}

/** The "@" menu, found by its own listbox name. */
const fileMenu = (): HTMLElement | null =>
  container.querySelector<HTMLElement>('[aria-label="Workspace files"]');

const fileRows = (): string[] =>
  [...(fileMenu()?.querySelectorAll<HTMLElement>('[role="option"]') ?? [])].map(
    (row) => row.textContent ?? "",
  );

/** Lets a pending index read settle inside act. */
async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  onSend = vi.fn<(text: string, attachments: readonly PromptAttachment[]) => Promise<boolean>>();
  onQueue = vi.fn<(text: string, attachments: readonly PromptAttachment[]) => void>();
  drive = composerDrivers(container);
});

afterEach(async () => {
  await act(async () => root?.unmount());
  container.remove();
  vi.clearAllMocks();
});

describe("the @ file menu", () => {
  it("reads the workspace index once, on the first @, and offers its files", async () => {
    const loadFileIndex = vi.fn(async () => ["src/App.tsx", "README.md", "src/app/index.ts"]);
    await renderComposer({ loadFileIndex });
    expect(loadFileIndex).not.toHaveBeenCalled();

    await drive.type("look at @app");
    await settle();
    expect(loadFileIndex).toHaveBeenCalledTimes(1);
    expect(fileMenu()).not.toBeNull();
    expect(fileRows()).toEqual(["@src/App.tsx", "@src/app/index.ts"]);

    await drive.type("look at @appx");
    await settle();
    expect(loadFileIndex).toHaveBeenCalledTimes(1);
  });

  it("puts the picked file into the text as its path, with a space after it", async () => {
    const loadFileIndex = async () => ["src/App.tsx", "README.md"];
    await renderComposer({ loadFileIndex });
    await drive.type("read @read");
    await settle();
    await drive.press("Enter");
    expect(drive.textarea().value).toBe("read @README.md ");
    expect(fileMenu()).toBeNull();
    expect(onSend).not.toHaveBeenCalled();
  });

  it("walks the files with the arrows and takes the highlighted one with Tab", async () => {
    await renderComposer({ loadFileIndex: async () => ["a.ts", "ab.ts", "abc.ts"] });
    await drive.type("@a");
    await settle();
    await drive.press("ArrowDown");
    await drive.press("Tab");
    expect(drive.textarea().value).toBe("@ab.ts ");
  });

  it("closes on Escape and keeps the text, and opens again on the next keystroke", async () => {
    await renderComposer({ loadFileIndex: async () => ["a.ts"] });
    await drive.type("@a");
    await settle();
    expect(fileMenu()).not.toBeNull();
    await drive.press("Escape");
    expect(fileMenu()?.querySelector('[role="option"]') ?? null).toBeNull();
    expect(drive.textarea().value).toBe("@a");
    await drive.type("@a.");
    await settle();
    expect(fileRows()).toEqual(["@a.ts"]);
  });

  it("says it is loading while the index is read, and that nothing matched", async () => {
    let finish: (files: readonly string[]) => void = () => undefined;
    const pending = new Promise<readonly string[]>((resolve) => {
      finish = resolve;
    });
    await renderComposer({ loadFileIndex: () => pending });
    await drive.type("@zz");
    await settle();
    expect(fileMenu()?.textContent).toContain("Loading files");
    await act(async () => {
      finish(["a.ts"]);
    });
    await settle();
    expect(fileMenu()?.textContent).toContain("No files found");
  });

  it("sends on Enter when no file matches, so a stray @ does not trap the draft", async () => {
    await renderComposer({ loadFileIndex: async () => ["a.ts"] });
    await drive.type("look @zzz");
    await settle();
    expect(fileMenu()).not.toBeNull();
    await drive.press("Enter");
    expect(onSend).toHaveBeenCalledTimes(1);
    expect(onSend.mock.calls[0]?.[0]).toBe("look @zzz");
  });
});
