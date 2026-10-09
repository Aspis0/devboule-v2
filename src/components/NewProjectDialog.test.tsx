// @vitest-environment happy-dom

// The add-project dialog's quiet open: the path hint shows single slashes it
// is impossible to misread as escapes, the folder button never wears the
// error tone on a pristine field, and the refusal appears only after a failed
// submit.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { NewProjectDialog } from "./NewProjectDialog";
import { assembleCssProof } from "../features/workspace/cssProof";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

describe("the add-project dialog", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  async function render(): Promise<void> {
    await act(async () => {
      root.render(<NewProjectDialog open onClose={() => {}} onCreate={() => {}} />);
    });
  }

  function input(): HTMLInputElement {
    const field = container.querySelector<HTMLInputElement>("#workspace-project-input");
    if (field === null) throw new Error("the path field did not render");
    return field;
  }

  it("hints an absolute path no layer can double", async () => {
    await render();

    expect(input().placeholder).toBe("C:/Users/you/project");
  });

  it("opens without any refusal showing", async () => {
    await render();

    expect(container.querySelector(".workspace-project-error")).toBeNull();
  });

  it("refuses an empty submit, and only then", async () => {
    await render();
    const form = container.querySelector<HTMLFormElement>("form");
    if (form === null) throw new Error("the dialog form did not render");

    await act(async () => {
      form.requestSubmit();
    });

    expect(container.querySelector(".workspace-project-error")?.textContent).toContain(
      "Choose or enter an absolute folder path.",
    );
  });

  it("never paints the folder button in the error tone", async () => {
    const proof = assembleCssProof([
      read("src/styles/tokens.css"),
      read("src/styles/global.css"),
      read("src/features/workspace/Workspace.css"),
    ]);
    const { rulesFor, token } = proof;

    // The shared secondary-action focus rule wears danger-deep; the picker
    // button opts back out to ink, so initial focus on open never reads as
    // an error.
    const rule = rulesFor(".workspace-project-picker-button:focus-visible");
    expect(rule, "a focus rule for the picker button is missing").not.toBe("");
    expect(rule).toContain(token("--ink")!);
    expect(rule).not.toContain(token("--danger-deep")!);
  });
});
