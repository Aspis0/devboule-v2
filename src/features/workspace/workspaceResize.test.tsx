// @vitest-environment happy-dom

import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import {
  INITIAL_LEFT_WIDTH,
  INITIAL_RIGHT_WIDTH,
  MAX_LEFT_WIDTH,
  MAX_RIGHT_WIDTH,
  MIN_LEFT_WIDTH,
  MIN_RIGHT_WIDTH,
  clampPanelWidth,
  readStoredPanelWidths,
  useWorkspacePanelResize,
  writeStoredPanelWidths,
  type StorageLike,
  type StoredPanelWidths,
} from "./workspaceResize";

function fakeStorage(initial: Record<string, string> = {}): StorageLike {
  const map = new Map(Object.entries(initial));
  return {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
  };
}

afterEach(() => {
  localStorage.clear();
});

describe("the shell frame's widths", () => {
  it("defaults to the spec: sidebar 248, right panel 300", () => {
    expect(INITIAL_LEFT_WIDTH).toBe(248);
    expect(INITIAL_RIGHT_WIDTH).toBe(300);
  });

  it("bounds each side separately: sidebar 200–360, right panel its own bounds", () => {
    expect(MIN_LEFT_WIDTH).toBe(200);
    expect(MAX_LEFT_WIDTH).toBe(360);
    expect(MIN_RIGHT_WIDTH).toBeLessThanOrEqual(INITIAL_RIGHT_WIDTH);
    expect(MAX_RIGHT_WIDTH).toBeGreaterThanOrEqual(INITIAL_RIGHT_WIDTH);
  });

  it("mounts when the localStorage getter itself throws", () => {
    const saved = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
    Object.defineProperty(globalThis, "localStorage", {
      configurable: true,
      get() {
        throw new Error("blocked");
      },
    });
    const holder = document.createElement("div");
    document.body.appendChild(holder);
    const root = createRoot(holder);
    try {
      function Probe() {
        const { leftWidth } = useWorkspacePanelResize();
        return <p data-testid="probe">{leftWidth}</p>;
      }
      act(() => {
        root.render(<Probe />);
      });
      expect(holder.querySelector("[data-testid=probe]")?.textContent).toBe(
        String(INITIAL_LEFT_WIDTH),
      );
      act(() => root.unmount());
    } finally {
      holder.remove();
      if (saved) {
        Object.defineProperty(globalThis, "localStorage", saved);
      } else {
        delete (globalThis as { localStorage?: unknown }).localStorage;
      }
    }
  });
});

describe("clampPanelWidth", () => {
  it("clamps out-of-bounds widths into their side's bounds, whichever side", () => {
    expect(clampPanelWidth(180, "left")).toBe(200);
    expect(clampPanelWidth(460, "left")).toBe(360);
    expect(clampPanelWidth(460, "right")).toBe(MAX_RIGHT_WIDTH);
    expect(clampPanelWidth(180, "right")).toBe(MIN_RIGHT_WIDTH);
  });

  it("keeps in-bounds widths and clamps both directions per side", () => {
    expect(clampPanelWidth(248, "left")).toBe(248);
    expect(clampPanelWidth(150, "left")).toBe(200);
    expect(clampPanelWidth(500, "left")).toBe(360);
    expect(clampPanelWidth(300, "right")).toBe(300);
    expect(clampPanelWidth(100, "right")).toBe(MIN_RIGHT_WIDTH);
    expect(clampPanelWidth(900, "right")).toBe(MAX_RIGHT_WIDTH);
  });
});

describe("persisted panel widths", () => {
  it("round-trips both sides", () => {
    const storage = fakeStorage();
    const widths: StoredPanelWidths = { left: 312, right: 344 };
    writeStoredPanelWidths(storage, widths);
    expect(readStoredPanelWidths(storage)).toEqual(widths);
  });

  it("reads nothing stored as the defaults", () => {
    expect(readStoredPanelWidths(fakeStorage())).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
    });
    expect(readStoredPanelWidths(null)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
    });
  });

  it("clamps a stored width that sits outside its side's bounds", () => {
    const storage = fakeStorage({
      "devboule.workspacePanelWidths": JSON.stringify({ left: 180, right: 459 }),
    });
    expect(readStoredPanelWidths(storage)).toEqual({ left: 200, right: MAX_RIGHT_WIDTH });
  });

  it("reads garbage, partial rows, or wrong types as the defaults, never as widths", () => {
    const garbage = fakeStorage({ "devboule.workspacePanelWidths": "{oops" });
    expect(readStoredPanelWidths(garbage)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
    });
    const partial = fakeStorage({
      "devboule.workspacePanelWidths": JSON.stringify({ left: 260 }),
    });
    expect(readStoredPanelWidths(partial)).toEqual({
      left: 260,
      right: INITIAL_RIGHT_WIDTH,
    });
    const wrongTypes = fakeStorage({
      "devboule.workspacePanelWidths": JSON.stringify({ left: "wide", right: null }),
    });
    expect(readStoredPanelWidths(wrongTypes)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
    });
  });

  it("survives a throwing store and a null store on both paths", () => {
    const throwing = {
      getItem: () => {
        throw new Error("blocked");
      },
      setItem: () => {
        throw new Error("full");
      },
    } as unknown as StorageLike;
    expect(readStoredPanelWidths(throwing)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
    });
    expect(() => writeStoredPanelWidths(throwing, { left: 248, right: 300 })).not.toThrow();
    expect(() => writeStoredPanelWidths(null, { left: 248, right: 300 })).not.toThrow();
  });
});
