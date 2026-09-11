// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ buildArtifactPrintDocument: vi.fn() }));

// The builder is mocked so one test can make document assembly throw, which is
// the failure a user cannot cause from outside. Everything else uses the real
// implementation, set once here from the original module.
vi.mock("./artifactPrint", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./artifactPrint")>();
  mocks.buildArtifactPrintDocument.mockImplementation(actual.buildArtifactPrintDocument);
  return { ...actual, buildArtifactPrintDocument: mocks.buildArtifactPrintDocument };
});

import {
  ARTIFACT_PRINT_CSP,
  ARTIFACT_PRINT_MESSAGE_KIND,
  ARTIFACT_PRINT_SANDBOX,
  ARTIFACT_PRINT_SOURCE,
} from "./artifactPrint";
import { ArtifactPrintControl } from "./ArtifactPrintControl";

const FRAGMENT = "<main><h1>Our menu</h1></main>";
const RUN_TITLE = "Agent did not report written files";

function printReport(status: "printed" | "failed", message?: string) {
  return {
    kind: ARTIFACT_PRINT_MESSAGE_KIND,
    source: ARTIFACT_PRINT_SOURCE,
    version: 1,
    status,
    ...(message === undefined ? {} : { message }),
  };
}

describe("ArtifactPrintControl", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    mocks.buildArtifactPrintDocument.mockClear();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    document.body
      .querySelectorAll(".design-artifact-print-frame")
      .forEach((frame) => frame.remove());
    vi.useRealTimers();
  });

  async function render(): Promise<void> {
    await act(async () => {
      root.render(<ArtifactPrintControl html={FRAGMENT} title={RUN_TITLE} />);
    });
  }

  function printButton(): HTMLButtonElement {
    const button = container.querySelector<HTMLButtonElement>('button[aria-label="Print / PDF"]');
    if (button === null) throw new Error("the print button is missing");
    return button;
  }

  function frameInBody(): HTMLIFrameElement | null {
    return document.body.querySelector<HTMLIFrameElement>(".design-artifact-print-frame");
  }

  async function click(): Promise<void> {
    await act(async () => {
      printButton().click();
    });
  }

  async function reportFrom(frame: HTMLIFrameElement, data: unknown): Promise<void> {
    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", { data, source: frame.contentWindow ?? undefined }),
      );
    });
  }

  it("offers one labelled button and says nothing before it is used", async () => {
    await render();

    expect(container.querySelectorAll("button")).toHaveLength(1);
    expect(printButton().textContent).toBe("Print / PDF");
    expect(printButton().title).toBe("Print the generated page, or save it as a PDF");
    expect(container.textContent).toBe("Print / PDF");
    expect(frameInBody()).toBeNull();
    expect(mocks.buildArtifactPrintDocument).not.toHaveBeenCalled();
  });

  it("mounts one sandboxed frame holding the print document, outside the pill", async () => {
    await render();

    await click();

    const frame = frameInBody();
    if (frame === null) throw new Error("the print frame did not mount");
    expect(mocks.buildArtifactPrintDocument).toHaveBeenCalledWith(FRAGMENT, RUN_TITLE);
    expect(frame.getAttribute("sandbox")).toBe(ARTIFACT_PRINT_SANDBOX);
    expect(frame.srcdoc).toContain(ARTIFACT_PRINT_CSP);
    expect(frame.srcdoc).toContain("window.print()");
    // Body-level on purpose: the frame is a full page, not a child of a
    // clipping/positioned pill, and the canvas pill must not be able to hide or
    // scale it.
    expect(container.querySelector(".design-artifact-print-frame")).toBeNull();
    expect(frame.isConnected).toBe(true);
  });

  it("removes the frame and says the dialog closed when the frame reports back", async () => {
    await render();
    await click();
    const frame = frameInBody();
    if (frame === null) throw new Error("the print frame did not mount");

    await reportFrom(frame, printReport("printed"));

    // The report is the end of the frame's life on the success path: whether
    // the user printed or cancelled, nothing of the document is kept.
    expect(frame.isConnected).toBe(false);
    expect(frameInBody()).toBeNull();
    expect(container.textContent).toBe("Print / PDFPrint dialog closed.");
    const status = container.querySelector('[role="status"]');
    expect(status?.textContent).toBe("Print dialog closed.");
  });

  it("reports a print the WebView refused, with the cause in the tooltip", async () => {
    await render();
    await click();
    const frame = frameInBody();
    if (frame === null) throw new Error("the print frame did not mount");

    await reportFrom(frame, printReport("failed", "This WebView does not implement printing."));

    expect(frameInBody()).toBeNull();
    expect(container.textContent).toBe("Print / PDFPrint failed.");
    const status = container.querySelector('[role="status"]');
    expect(status?.textContent).toBe("Print failed.");
    expect(status?.getAttribute("title")).toBe("This WebView does not implement printing.");
  });

  it("ignores reports that did not come from its own frame", async () => {
    await render();
    await click();
    const frame = frameInBody();
    if (frame === null) throw new Error("the print frame did not mount");

    // Wrong window, then right window with a malformed payload. Neither ends a
    // print that is still in flight.
    await act(async () => {
      window.dispatchEvent(
        new MessageEvent("message", { data: printReport("printed"), source: window }),
      );
    });
    await reportFrom(frame, { ...printReport("printed"), kind: "something-else" });

    expect(frameInBody()).toBe(frame);
    expect(container.textContent).toBe("Print / PDF");

    await reportFrom(frame, printReport("printed"));
    expect(frameInBody()).toBeNull();
  });

  it("says Print failed. and leaves no frame when the document cannot be built", async () => {
    mocks.buildArtifactPrintDocument.mockImplementationOnce(() => {
      throw new Error(
        "The standalone export must carry the canvas policy (ARTIFACT_CSP) in exactly one meta tag",
      );
    });
    await render();

    await click();

    expect(frameInBody()).toBeNull();
    expect(container.textContent).toBe("Print / PDFPrint failed.");
    expect(container.querySelector('[role="status"]')?.getAttribute("title")).toBe(
      "The standalone export must carry the canvas policy (ARTIFACT_CSP) in exactly one meta tag",
    );
  });

  it("replaces the frame instead of stacking one when clicked again mid-print", async () => {
    await render();

    await click();
    const first = frameInBody();
    if (first === null) throw new Error("the print frame did not mount");
    await click();

    // One frame at a time, and the live one is the newest: a second click must
    // never leave two artifact documents in the body, and must never be
    // swallowed without a dialog (the state the button cannot recover from).
    const second = frameInBody();
    expect(second).not.toBe(first);
    expect(first.isConnected).toBe(false);
    expect(document.body.querySelectorAll(".design-artifact-print-frame")).toHaveLength(1);
    expect(mocks.buildArtifactPrintDocument).toHaveBeenCalledTimes(2);
  });

  it("takes the frame with it when the control unmounts mid-print", async () => {
    await render();
    await click();
    const frame = frameInBody();
    if (frame === null) throw new Error("the print frame did not mount");

    await act(async () => root.unmount());

    // An unmount during a print used to be the leak: a full artifact document
    // left in the body with nobody left to remove it.
    expect(frame.isConnected).toBe(false);
    expect(frameInBody()).toBeNull();
    // The afterEach above unmounts again; give it an empty root rather than a
    // spent one.
    root = createRoot(container);
  });

  it("retires Print dialog closed. after its reading window", async () => {
    vi.useFakeTimers();
    await render();
    await click();
    const frame = frameInBody();
    if (frame === null) throw new Error("the print frame did not mount");

    await reportFrom(frame, printReport("printed"));
    expect(container.textContent).toContain("Print dialog closed.");

    await act(async () => {
      vi.advanceTimersByTime(4_000);
    });

    expect(container.textContent).toBe("Print / PDF");
  });
});
