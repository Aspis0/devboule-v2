// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act } from "react";
import { createRoot } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import {
  INITIAL_LEFT_WIDTH,
  INITIAL_RIGHT_WIDTH,
  MAX_LEFT_WIDTH,
  MAX_RIGHT_WIDTH,
  MIN_CENTER_WIDTH,
  MIN_LEFT_WIDTH,
  MIN_RIGHT_WIDTH,
  clampPanelWidth,
  readStoredPanelFrame,
  useWorkspacePanelResize,
  writeStoredPanelFrame,
  type StorageLike,
  type StoredPanelFrame,
} from "./workspaceResize";

const RECORD = "devboule.workspacePanelWidths";

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
  it("defaults to the spec: sidebar 320, right panel 300", () => {
    expect(INITIAL_LEFT_WIDTH).toBe(320);
    expect(INITIAL_RIGHT_WIDTH).toBe(300);
  });

  it("bounds each side separately: sidebar 200–600, right panel its own bounds", () => {
    expect(MIN_LEFT_WIDTH).toBe(200);
    expect(MAX_LEFT_WIDTH).toBe(600);
    expect(MIN_RIGHT_WIDTH).toBeLessThanOrEqual(INITIAL_RIGHT_WIDTH);
    expect(MAX_RIGHT_WIDTH).toBeGreaterThanOrEqual(INITIAL_RIGHT_WIDTH);
  });

  it("takes the sidebar's bounds from the geometry tokens, so CSS and the clamp cannot drift", () => {
    const sheet = readFileSync(resolve(import.meta.dirname, "../../styles/tokens.css"), "utf8");
    const token = (name: string): number => {
      const match = new RegExp(`--${name}:\\s*(\\d+)px;`).exec(sheet);
      expect(match, `--${name} is missing from tokens.css`).not.toBeNull();
      return Number.parseInt(match![1]!, 10);
    };
    expect(token("sidebar-width")).toBe(INITIAL_LEFT_WIDTH);
    expect(token("sidebar-min")).toBe(MIN_LEFT_WIDTH);
    expect(token("sidebar-max")).toBe(MAX_LEFT_WIDTH);
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
    expect(clampPanelWidth(700, "left")).toBe(600);
    expect(clampPanelWidth(460, "right")).toBe(MAX_RIGHT_WIDTH);
    expect(clampPanelWidth(180, "right")).toBe(MIN_RIGHT_WIDTH);
  });

  it("keeps in-bounds widths and clamps both directions per side", () => {
    expect(clampPanelWidth(248, "left")).toBe(248);
    expect(clampPanelWidth(150, "left")).toBe(200);
    expect(clampPanelWidth(500, "left")).toBe(500);
    expect(clampPanelWidth(300, "right")).toBe(300);
    expect(clampPanelWidth(100, "right")).toBe(MIN_RIGHT_WIDTH);
    expect(clampPanelWidth(900, "right")).toBe(MAX_RIGHT_WIDTH);
  });

  it("gives the centre its floor at 1024px, on whichever side asks", () => {
    // The screen draws two 6px handles between the panels and the centre, so
    // the window owes the other panel 300, the centre 360 and the pair of
    // handles 12: 1024 − 300 − 360 − 12 = 352, never the 600 the rail allows.
    const room = { other: 300, viewport: 1024 };
    const left = clampPanelWidth(MAX_LEFT_WIDTH, "left", room);
    expect(left).toBe(352);
    expect(left + 12 + MIN_CENTER_WIDTH + room.other).toBe(room.viewport);

    const rightRoom = { other: 320, viewport: 1024 };
    const right = clampPanelWidth(MAX_RIGHT_WIDTH, "right", rightRoom);
    expect(right + 12 + MIN_CENTER_WIDTH + rightRoom.other).toBe(rightRoom.viewport);
  });

  it("keeps the side's own minimum when the window is smaller than both minima", () => {
    expect(clampPanelWidth(MAX_LEFT_WIDTH, "left", { other: 400, viewport: 700 })).toBe(
      MIN_LEFT_WIDTH,
    );
  });

  it("leaves the bounds alone when no window is in question", () => {
    expect(clampPanelWidth(MAX_LEFT_WIDTH, "left")).toBe(MAX_LEFT_WIDTH);
    expect(clampPanelWidth(0, "left", { other: 300, viewport: 0 })).toBe(MIN_LEFT_WIDTH);
  });
});

describe("persisted panel widths", () => {
  it("round-trips both sides", () => {
    const storage = fakeStorage();
    const frame: StoredPanelFrame = {
      left: 312,
      right: 344,
      leftCollapsed: false,
      rightCollapsed: false,
    };
    writeStoredPanelFrame(storage, frame);
    expect(readStoredPanelFrame(storage)).toEqual(frame);
  });

  it("reads nothing stored as the defaults", () => {
    expect(readStoredPanelFrame(fakeStorage())).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
    });
    expect(readStoredPanelFrame(null)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
    });
  });

  it("clamps a stored width that sits outside its side's bounds", () => {
    const storage = fakeStorage({
      [RECORD]: JSON.stringify({ left: 180, right: 459 }),
    });
    expect(readStoredPanelFrame(storage)).toEqual({
      left: 200,
      right: MAX_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
    });
  });

  it("reads garbage, partial rows, or wrong types as the defaults, never as widths", () => {
    const garbage = fakeStorage({ [RECORD]: "{oops" });
    expect(readStoredPanelFrame(garbage)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
    });
    const partial = fakeStorage({
      [RECORD]: JSON.stringify({ left: 260 }),
    });
    expect(readStoredPanelFrame(partial)).toEqual({
      left: 260,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
    });
    const wrongTypes = fakeStorage({
      [RECORD]: JSON.stringify({ left: "wide", right: null }),
    });
    expect(readStoredPanelFrame(wrongTypes)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
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
    expect(readStoredPanelFrame(throwing)).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
    });
    const frame: StoredPanelFrame = {
      left: 248,
      right: 300,
      leftCollapsed: false,
      rightCollapsed: false,
    };
    expect(() => writeStoredPanelFrame(throwing, frame)).not.toThrow();
    expect(() => writeStoredPanelFrame(null, frame)).not.toThrow();
  });
});

/** The hook's four fields on screen; the buttons are the routes the shell
 * wires — menu collapse, strip expand, sidebar collapse. */
function Probe() {
  const {
    leftWidth,
    rightWidth,
    leftMax,
    leftCollapsed,
    rightCollapsed,
    setLeftCollapsed,
    setRightCollapsed,
  } = useWorkspacePanelResize();
  return (
    <div>
      <p data-testid="frame">{`${leftWidth}/${rightWidth}/${leftCollapsed}/${rightCollapsed}`}</p>
      <p data-testid="room">{`${leftMax}`}</p>
      <button type="button" data-testid="collapse-right" onClick={() => setRightCollapsed(true)}>
        collapse
      </button>
      <button type="button" data-testid="expand-right" onClick={() => setRightCollapsed(false)}>
        expand
      </button>
      <button type="button" data-testid="collapse-left" onClick={() => setLeftCollapsed(true)}>
        sidebar
      </button>
    </div>
  );
}

describe("the persisted collapsed flags", () => {
  let holder: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;

  beforeEach(() => {
    localStorage.clear();
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    holder.remove();
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(<Probe />);
    });
  }

  async function remount(): Promise<void> {
    await act(async () => root.unmount());
    root = createRoot(holder);
    await mount();
  }

  function frame(): string | undefined {
    return holder.querySelector("[data-testid=frame]")?.textContent ?? undefined;
  }

  function click(testid: string): Promise<void> {
    const button = holder.querySelector<HTMLButtonElement>(`[data-testid=${testid}]`);
    if (button === null) throw new Error(`${testid} did not render`);
    return act(async () => {
      button.click();
    });
  }

  it("keeps the panel collapsed across a remount, and reopens it on demand", async () => {
    await mount();
    expect(frame()).toBe("320/300/false/false");

    await click("collapse-right");
    expect(frame()).toBe("320/300/false/true");
    expect(JSON.parse(localStorage.getItem(RECORD) ?? "null")).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: true,
    });
    await remount();
    expect(frame()).toBe("320/300/false/true");

    await click("expand-right");
    expect(frame()).toBe("320/300/false/false");
    await remount();
    expect(frame()).toBe("320/300/false/false");
  });

  it("remembers the sidebar's collapse in the same record: one hook, one key", async () => {
    await mount();
    await click("collapse-left");
    expect(frame()).toBe("320/300/true/false");
    await remount();
    expect(frame()).toBe("320/300/true/false");
    const stored = JSON.parse(localStorage.getItem(RECORD) ?? "null");
    expect(stored.leftCollapsed).toBe(true);
    expect(stored.rightCollapsed).toBe(false);
  });

  it("reads the width the default once stored as the default it now is", () => {
    // 248 was never a drag's answer, so it takes today's default; a width the
    // user did choose stays exactly as they left it.
    const storedDefault = fakeStorage({ [RECORD]: JSON.stringify({ left: 248, right: 300 }) });
    expect(readStoredPanelFrame(storedDefault).left).toBe(INITIAL_LEFT_WIDTH);
    const chosen = fakeStorage({ [RECORD]: JSON.stringify({ left: 340, right: 300 }) });
    expect(readStoredPanelFrame(chosen).left).toBe(340);
  });

  it("reads a legacy record without the flags as open, with its widths intact", () => {
    const storage = fakeStorage({ [RECORD]: JSON.stringify({ left: 312, right: 344 }) });
    expect(readStoredPanelFrame(storage)).toEqual({
      left: 312,
      right: 344,
      leftCollapsed: false,
      rightCollapsed: false,
    });
  });

  it("reads a non-boolean flag as open, with its widths intact", () => {
    const storage = fakeStorage({
      [RECORD]: JSON.stringify({ left: 312, right: 344, rightCollapsed: "yes" }),
    });
    expect(readStoredPanelFrame(storage)).toEqual({
      left: 312,
      right: 344,
      leftCollapsed: false,
      rightCollapsed: false,
    });
  });

  it("reads a malformed record as open, with the default widths", () => {
    expect(readStoredPanelFrame(fakeStorage({ [RECORD]: "not json" }))).toEqual({
      left: INITIAL_LEFT_WIDTH,
      right: INITIAL_RIGHT_WIDTH,
      leftCollapsed: false,
      rightCollapsed: false,
    });
  });
});

describe("the window's rule on the drawn widths", () => {
  let holder: HTMLDivElement;
  let root: ReturnType<typeof createRoot>;
  const realInnerWidth = window.innerWidth;

  function setWindowWidth(width: number): void {
    Object.defineProperty(window, "innerWidth", { configurable: true, value: width });
  }

  beforeEach(() => {
    localStorage.clear();
    setWindowWidth(1024);
    holder = document.createElement("div");
    document.body.appendChild(holder);
    root = createRoot(holder);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    holder.remove();
    setWindowWidth(realInnerWidth);
  });

  async function mount(): Promise<void> {
    await act(async () => {
      root.render(<Probe />);
    });
  }

  function shown(): string | undefined {
    return holder.querySelector("[data-testid=frame]")?.textContent ?? undefined;
  }

  function handleMax(): string | undefined {
    return holder.querySelector("[data-testid=room]")?.textContent ?? undefined;
  }

  it("draws a 600px rail as 352 at 1024px, and the whole rail once the window grows", async () => {
    localStorage.setItem(
      RECORD,
      JSON.stringify({ left: 600, right: 300, leftCollapsed: false, rightCollapsed: false }),
    );
    await mount();

    expect(shown()).toBe("352/300/false/false");
    // The handle may not promise more than the window can show.
    expect(handleMax()).toBe("352");

    // The record still holds 600 — only what is drawn was clipped — so the
    // rail comes back whole when the window does.
    setWindowWidth(1440);
    await act(async () => {
      window.dispatchEvent(new Event("resize"));
    });
    expect(shown()).toBe("600/300/false/false");
    expect(handleMax()).toBe("600");
  });
});
