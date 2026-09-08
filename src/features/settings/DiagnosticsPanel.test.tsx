// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  return {
    ...actual,
    daemonDiagnostics: vi.fn(),
  };
});

import { daemonDiagnostics } from "../../lib/tauri";
import type { DaemonDiagnostics } from "../../types/ipc";
import {
  DiagnosticsErrorBoundary,
  DiagnosticsPanel,
  formatDiagnostics,
  loadDiagnostics,
} from "./DiagnosticsPanel";
import diagnosticsFixture from "../../../crates/devboule-daemon/fixtures/diagnostics-report.json";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// This import is the same committed fixture generated and round-tripped by
// the Rust diagnostics test. It keeps the frontend test on the real wire.
function parseCaptureState(
  value: string,
): DaemonDiagnostics["environment"]["loginShellCapture"]["state"] {
  switch (value) {
    case "not_run":
    case "applied":
    case "skipped":
    case "failed":
      return value;
    default:
      throw new Error(`Unexpected login-shell capture state in fixture: ${value}`);
  }
}
const captureState = parseCaptureState(diagnosticsFixture.environment.loginShellCapture.state);
const sampleReport: DaemonDiagnostics = {
  ...diagnosticsFixture,
  environment: {
    ...diagnosticsFixture.environment,
    loginShellCapture: {
      ...diagnosticsFixture.environment.loginShellCapture,
      state: captureState,
    },
  },
};

const emptyReport = {
  daemon: {},
  health: {},
  sessions: {},
  providers: [],
  environment: {},
} as unknown as DaemonDiagnostics;

describe("diagnostics panel", () => {
  let container: HTMLDivElement;
  let root: Root;
  const clipboardWrites: string[] = [];
  const realClipboard = navigator.clipboard;

  function stubClipboard(): void {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: vi.fn(async (text: string) => {
          clipboardWrites.push(text);
        }),
      },
    });
    clipboardWrites.length = 0;
  }

  function renderPanel(): void {
    root.render(<DiagnosticsPanel />);
  }

  async function clickCopy(): Promise<void> {
    const button = container.querySelector<HTMLButtonElement>(".diagnostics-copy");
    if (button === null) throw new Error("copy button did not render");
    await act(async () => button.click());
    await act(async () => undefined);
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    stubClipboard();
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: realClipboard });
    vi.useRealTimers();
    vi.clearAllMocks();
  });

  it("says precisely what the report does and does not contain", async () => {
    vi.mocked(daemonDiagnostics).mockResolvedValue(sampleReport);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);

    expect(daemonDiagnostics).toHaveBeenCalledTimes(1);
    const text = container.textContent ?? "";
    expect(text).toContain("Diagnostics");
    // The safety sentence must be exact: the report DOES carry paths (the
    // runtime dir and pipe name, with the home directory redacted), so "no
    // file paths" would be a lie the user can catch by reading the paste.
    expect(text).toContain("no secrets");
    expect(text).toContain("no conversation content");
    expect(text).toContain("no session titles");
    expect(text).toContain("home directory");
    expect(text).not.toContain("no file paths");
    // Labelled wire fields, not raw keys.
    expect(text).toContain("uptime ms");
    expect(text).toContain("20651");
    expect(text).toContain("0.1.0");
    expect(text).toContain("claude");
    // The formatted text block is visible for manual copying.
    expect(container.querySelector(".diagnostics-text")?.textContent).toContain("== daemon ==");
  });

  it("renders the documented camelCase field names as labelled rows", async () => {
    vi.mocked(daemonDiagnostics).mockResolvedValue(sampleReport);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);

    const text = container.textContent ?? "";
    expect(text).toContain("uptime ms");
    expect(text).toContain("capabilities");
    expect(text).toContain("ping, status");
    expect(text).toContain("accepted frames");
    expect(text).toContain("journal schema version");
    expect(text).toContain("live");
    expect(text).toContain("runtime dir");
    expect(text).toContain("[redacted-home]");
    expect(text).toContain("pipe name");
    expect(text).toContain("install channel");
  });

  it("renders journal facts as a separate readable section", async () => {
    vi.mocked(daemonDiagnostics).mockResolvedValue(sampleReport);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);

    const sections = [...container.querySelectorAll(".diagnostics-section")];
    const journal = sections.find((section) => section.textContent?.includes("Journal"));
    if (journal === undefined) throw new Error("journal section did not render");
    expect(journal.textContent).toContain("accepted frames");
    expect(journal.textContent).toContain("journal file bytes");
    expect(journal.textContent).not.toContain("[object Object]");
  });

  it("copying puts the formatted text on the clipboard", async () => {
    vi.mocked(daemonDiagnostics).mockResolvedValue(sampleReport);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);
    await clickCopy();

    expect(clipboardWrites).toHaveLength(1);
    const copied = clipboardWrites[0] ?? "";
    expect(copied.startsWith("devboule diagnostics")).toBe(true);
    expect(copied).toContain("== daemon ==");
    expect(copied).toContain("version: 0.1.0");
    expect(copied).toContain("accepted frames: 0");
    expect(copied).toContain("claude");
  });

  it("clears Copied after a short delay and resets the delay on repeat clicks", async () => {
    vi.useFakeTimers();
    vi.mocked(daemonDiagnostics).mockResolvedValue(sampleReport);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);

    await clickCopy();
    expect(container.textContent).toContain("Copied.");
    await act(async () => vi.advanceTimersByTime(1_000));
    await clickCopy();
    await act(async () => vi.advanceTimersByTime(1_000));
    expect(container.textContent).toContain("Copied.");
    await act(async () => vi.advanceTimersByTime(1_000));
    expect(container.textContent).not.toContain("Copied.");
  });

  it("cleans the copy reset timer when the panel unmounts", async () => {
    vi.useFakeTimers();
    const clearTimeoutSpy = vi.spyOn(globalThis, "clearTimeout");
    vi.mocked(daemonDiagnostics).mockResolvedValue(sampleReport);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);
    await clickCopy();

    await act(async () => root.unmount());
    expect(clearTimeoutSpy).toHaveBeenCalled();
    clearTimeoutSpy.mockRestore();
  });

  it("two renders of the same report produce identical copy text", async () => {
    vi.mocked(daemonDiagnostics).mockResolvedValue(sampleReport);

    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);
    await clickCopy();
    const first = clipboardWrites[0];
    await act(async () => root.unmount());

    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);
    await clickCopy();
    const second = clipboardWrites[1];
    await act(async () => root.unmount());

    expect(first).toBeDefined();
    expect(second).toBeDefined();
    expect(second).toBe(first);
  });

  it("a failing command shows an error state, not an empty report", async () => {
    vi.mocked(daemonDiagnostics).mockRejectedValue(new Error("the daemon is not answering"));
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);

    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("error state did not render");
    expect(alert.textContent).toContain("the daemon is not answering");
    expect(container.textContent).not.toContain("no diagnostics data");
    expect(container.querySelector(".diagnostics-copy")).toBeNull();
  });

  it("an empty-but-successful report is distinguishable from a failure", async () => {
    vi.mocked(daemonDiagnostics).mockResolvedValue(emptyReport);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);

    expect(container.textContent).toContain("no diagnostics data");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });

  it("keeps the app mounted with a visible message for a malformed report", async () => {
    vi.mocked(daemonDiagnostics).mockResolvedValue({
      ...sampleReport,
      health: null,
    } as unknown as DaemonDiagnostics);
    root = createRoot(container);
    await act(async () => renderPanel());
    await act(async () => undefined);

    const alert = container.querySelector('[role="alert"]');
    if (alert === null) throw new Error("malformed report unmounted the panel");
    expect(alert.textContent).toContain("invalid diagnostics report");
    expect(container.querySelector("#settings-panel-diagnostics")).not.toBeNull();
  });

  it("can reset the error boundary and render a good child", async () => {
    let broken = true;
    function Child() {
      if (broken) throw new Error("render failed");
      return <p>healthy diagnostics child</p>;
    }

    root = createRoot(container);
    await act(async () => {
      root.render(
        <DiagnosticsErrorBoundary>
          <Child />
        </DiagnosticsErrorBoundary>,
      );
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toContain("render failed");
    const retry = container.querySelector<HTMLButtonElement>(".diagnostics-boundary-retry");
    if (retry === null) throw new Error("boundary retry control did not render");
    broken = false;
    await act(async () => retry.click());
    expect(container.textContent).toContain("healthy diagnostics child");
  });
});

describe("formatDiagnostics", () => {
  it("is deterministic for the same report regardless of key order", () => {
    const a = formatDiagnostics(sampleReport);
    const b = formatDiagnostics({
      ...sampleReport,
      health: {
        journalFileBytes: sampleReport.health.journalFileBytes,
        journalSchemaVersion: sampleReport.health.journalSchemaVersion,
        journalStats: sampleReport.health.journalStats,
        ringDroppedFrames: sampleReport.health.ringDroppedFrames,
        ringEvictedBytes: sampleReport.health.ringEvictedBytes,
        peakRingBytes: sampleReport.health.peakRingBytes,
      },
    });
    expect(b).toBe(a);
  });

  it("accepts the Rust-generated fixture as the frontend diagnostics type", () => {
    expect(formatDiagnostics(sampleReport)).toContain("== journal ==");
    expect(formatDiagnostics(sampleReport)).not.toContain("[object Object]");
  });

  it("cancelling a diagnostics load suppresses late success and failure callbacks", async () => {
    let resolveReport: ((report: DaemonDiagnostics) => void) | undefined;
    vi.mocked(daemonDiagnostics).mockImplementation(
      () =>
        new Promise<DaemonDiagnostics>((resolve) => {
          resolveReport = resolve;
        }),
    );
    const onReport = vi.fn();
    const onError = vi.fn();
    const cancel = loadDiagnostics(onReport, onError);
    cancel();
    resolveReport?.(sampleReport);
    await act(async () => undefined);

    expect(onReport).not.toHaveBeenCalled();
    expect(onError).not.toHaveBeenCalled();
  });
});
