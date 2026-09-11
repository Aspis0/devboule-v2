// @vitest-environment happy-dom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { buildStandaloneArtifactHtml } from "./artifactExport";

const mocks = vi.hoisted(() => ({
  save: vi.fn(),
  writeArtifactFile: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({ save: mocks.save }));

vi.mock("../../lib/tauri", () => ({
  writeArtifactFile: mocks.writeArtifactFile,
  reasonFromCause: (cause: unknown) => (cause instanceof Error ? cause.message : String(cause)),
}));

import { artifactFileName, saveArtifactHtml } from "./artifactSave";

const FRAGMENT = "<main><h1>Our menu</h1><p>Hi</p></main>";
const RUN_TITLE = "Agent did not report written files";

describe("artifact save flow", () => {
  beforeEach(() => {
    mocks.save.mockReset();
    mocks.writeArtifactFile.mockReset();
  });

  it("writes the exact bytes the Copy HTML button would copy, with no post-processing", async () => {
    // The contract: "Copy HTML" (DesignSurface.tsx) calls
    //   navigator.clipboard.writeText(buildStandaloneArtifactHtml(html, title))
    // and the file must hold that same string. Both sides compute the document
    // from one call, so the only way they can diverge is if the save path
    // transforms it afterwards. This test pins the string handed to the writer
    // to the exporter's own output, byte for byte, starting doctype and
    // trailing newline included.
    const copied = buildStandaloneArtifactHtml(FRAGMENT, RUN_TITLE);
    mocks.save.mockResolvedValue("C:/tmp/Our menu.html");
    mocks.writeArtifactFile.mockResolvedValue("C:/tmp/Our menu.html");

    const outcome = await saveArtifactHtml(FRAGMENT, RUN_TITLE);

    expect(outcome).toEqual({ status: "saved", path: "C:/tmp/Our menu.html" });
    const written = mocks.writeArtifactFile.mock.calls[0]?.[1];
    expect(written).toBe(copied);
    expect(written.startsWith("<!DOCTYPE html>\n")).toBe(true);
    expect(written.endsWith("\n")).toBe(true);
  });

  it("reports the path the write committed, not the one the dialog offered", async () => {
    mocks.save.mockResolvedValue("C:/tmp/typed-by-hand.html");
    mocks.writeArtifactFile.mockResolvedValue("C:/tmp/resolved-by-the-backend.html");

    const outcome = await saveArtifactHtml(FRAGMENT, RUN_TITLE);

    expect(outcome).toEqual({ status: "saved", path: "C:/tmp/resolved-by-the-backend.html" });
  });

  it("proposes a file name derived from the page title", async () => {
    mocks.save.mockResolvedValue("C:/tmp/Our menu.html");
    mocks.writeArtifactFile.mockResolvedValue("C:/tmp/Our menu.html");

    await saveArtifactHtml(FRAGMENT, RUN_TITLE);

    expect(mocks.save).toHaveBeenCalledWith({
      title: "Save the generated page",
      defaultPath: "Our menu.html",
      filters: [{ name: "HTML document", extensions: ["html"] }],
    });
  });

  it("returns cancelled, not failed, when the dialog closes with no choice", async () => {
    mocks.save.mockResolvedValue(null);

    const outcome = await saveArtifactHtml(FRAGMENT, RUN_TITLE);

    expect(outcome).toEqual({ status: "cancelled" });
    expect(mocks.writeArtifactFile).not.toHaveBeenCalled();
  });

  it("reports a dialog failure as failed", async () => {
    mocks.save.mockRejectedValue(new Error("the dialog host is gone"));

    const outcome = await saveArtifactHtml(FRAGMENT, RUN_TITLE);

    expect(outcome).toEqual({ status: "failed", message: "the dialog host is gone" });
    expect(mocks.writeArtifactFile).not.toHaveBeenCalled();
  });

  it("keeps a write failure distinct from a cancellation", async () => {
    mocks.save.mockResolvedValue("C:/tmp/report.html");
    mocks.writeArtifactFile.mockRejectedValue(
      new Error("writing `C:/tmp/report.html` failed: Access is denied. (os error 5)"),
    );

    const outcome = await saveArtifactHtml(FRAGMENT, RUN_TITLE);

    expect(outcome).toEqual({
      status: "failed",
      message: "writing `C:/tmp/report.html` failed: Access is denied. (os error 5)",
    });
    expect(outcome.status).not.toBe("cancelled");
  });
});

describe("artifactFileName", () => {
  it("names the file after the page's own title", () => {
    const documentHtml = buildStandaloneArtifactHtml(FRAGMENT, RUN_TITLE);
    expect(artifactFileName(documentHtml)).toBe("Our menu.html");
  });

  it("uses the page h1 over the run title, like the exported document does", () => {
    const documentHtml = buildStandaloneArtifactHtml(FRAGMENT, "Edited Index header");
    expect(artifactFileName(documentHtml)).toBe("Our menu.html");
  });

  it("replaces characters a file name cannot carry", () => {
    const documentHtml = buildStandaloneArtifactHtml(
      '<main><h1>Q3/Q4: "plan" &amp; <b>more</b></h1></main>',
      RUN_TITLE,
    );
    expect(artifactFileName(documentHtml)).toBe("Q3-Q4- -plan- & more.html");
  });

  it("strips a trailing dot Windows would drop in the dialog", () => {
    const documentHtml = buildStandaloneArtifactHtml("<main><h1>Report.</h1></main>", RUN_TITLE);
    expect(artifactFileName(documentHtml)).toBe("Report.html");
  });

  it("bounds a runaway title at 80 characters", () => {
    const documentHtml = buildStandaloneArtifactHtml(
      `<main><h1>${"a".repeat(200)}</h1></main>`,
      RUN_TITLE,
    );
    const name = artifactFileName(documentHtml);
    expect(name.endsWith(".html")).toBe(true);
    expect(name.slice(0, -".html".length)).toHaveLength(80);
  });

  it("falls back to a usable name when nothing nameable is left", () => {
    const documentHtml = buildStandaloneArtifactHtml("<main><h1>...</h1></main>", RUN_TITLE);
    expect(artifactFileName(documentHtml)).toBe("artifact.html");
  });
});
