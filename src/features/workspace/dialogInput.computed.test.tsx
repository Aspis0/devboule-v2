// @vitest-environment happy-dom

// The dialog's text-field rule in Workspace.css reaches text fields only: a
// radio stretched to full width squeezes its label into a wrapped column.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "./cssProof";

const rootDir = resolve(import.meta.dirname, "../../..");

function read(path: string): string {
  return readFileSync(resolve(rootDir, path), "utf8");
}

const proof = assembleCssProof([
  read("src/styles/tokens.css"),
  read("src/styles/global.css"),
  read("src/features/workspace/Workspace.css"),
]);

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("the dialog's text-field rule (live stylesheets)", () => {
  it("stretches a text field to the dialog width and leaves a radio at its own size", () => {
    proof.inject(['.workspace-project-dialog input:not([type="radio"])']);
    const dialog = document.createElement("div");
    dialog.className = "workspace-project-dialog";
    const radio = document.createElement("input");
    radio.type = "radio";
    const text = document.createElement("input");
    text.type = "text";
    dialog.append(radio, text);
    document.body.appendChild(dialog);

    expect(getComputedStyle(text).width).toBe("100%");
    expect(getComputedStyle(radio).width).not.toBe("100%");
  });
});
