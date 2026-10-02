// @vitest-environment happy-dom

// Every handler that owns an app-level key runs the keymap's matchers. Each
// test first answers a matcher for a key it rejects — the handler must act —
// then rejects the real binding — the handler must stay still. A handler that
// keeps a local switch for the real key fails the second half. The mock passes
// every binding it is not told about through to the real module.
import { act, useRef, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../types/surface", async () => {
  const actual = await vi.importActual<typeof import("../../types/surface")>("../../types/surface");
  const extras = [
    ["extra-one", "Extra One"],
    ["extra-two", "Extra Two"],
    ["extra-three", "Extra Three"],
    ["extra-four", "Extra Four"],
    ["extra-five", "Extra Five"],
    ["extra-six", "Extra Six"],
    ["extra-seven", "Extra Seven"],
  ].map(([key, label]) => ({
    key: key as SurfaceKey,
    label,
    eyebrow: "test fixture",
    description: "A named extra surface used only to exercise the paging window.",
    tone: "ochre" as const,
  }));
  return { ...actual, SURFACES: [...actual.SURFACES, ...extras] };
});

const wiring = vi.hoisted(() => ({
  stripChord: null as null | (() => "next" | "previous" | null),
  tabMove: null as null | (() => "next" | "previous" | "first" | "last" | null),
  closeTab: null as null | (() => boolean),
  composerAction: null as null | (() => "submit" | "alternate" | "newline" | null),
  crescentPage: null as null | (() => "next" | "previous" | null),
}));

vi.mock("../../lib/keymap", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/keymap")>();
  return {
    ...actual,
    stripChordFor: (event: Parameters<typeof actual.stripChordFor>[0]) =>
      wiring.stripChord === null ? actual.stripChordFor(event) : wiring.stripChord(),
    tabMoveForKey: (key: string) =>
      wiring.tabMove === null ? actual.tabMoveForKey(key) : wiring.tabMove(),
    isCloseTabKey: (key: string) =>
      wiring.closeTab === null ? actual.isCloseTabKey(key) : wiring.closeTab(),
    composerKeyAction: (event: Parameters<typeof actual.composerKeyAction>[0]) =>
      wiring.composerAction === null ? actual.composerKeyAction(event) : wiring.composerAction(),
    crescentPageForKey: (key: string) =>
      wiring.crescentPage === null ? actual.crescentPageForKey(key) : wiring.crescentPage(),
  };
});

import { Shell } from "../../app/Shell";
import { useAppStore } from "../../store/appStore";
import type { SurfaceKey } from "../../types/surface";
import { composerDrivers, composerProps, type ComposerMocks } from "./composerTestKit";
import { usePanelTabsKeyboard } from "./panel/usePanelTabsKeyboard";
import { useStripKeyboard } from "./strip/useStripKeyboard";
import { WorkspaceComposer } from "./WorkspaceComposer";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const OTHER_KEY = "x";

let container: HTMLDivElement;
let root: Root;

async function render(node: ReactNode): Promise<void> {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => root.render(node));
}

async function press(
  target: EventTarget,
  key: string,
  modifiers: KeyboardEventInit = {},
): Promise<void> {
  await act(async () => {
    target.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...modifiers }),
    );
  });
}

function StripHarness({
  selectTab,
  closeTab,
}: {
  selectTab: (id: string) => void;
  closeTab: (id: string) => void;
}) {
  const { tabIndexFor, onChipKeyDown } = useStripKeyboard({
    tabs: [{ id: "a" }, { id: "b" }],
    activeTabId: "a",
    selectTab,
    closeTab,
  });
  return (
    <div>
      {["a", "b"].map((id) => (
        <button
          key={id}
          data-chip={id}
          tabIndex={tabIndexFor(id)}
          onKeyDown={(event) => onChipKeyDown(id, event)}
        >
          {id}
        </button>
      ))}
    </div>
  );
}

function PanelHarness({ onSelect }: { onSelect: (id: string) => void }) {
  const listRef = useRef<HTMLDivElement>(null);
  const { tabIndexFor, onTabKeyDown } = usePanelTabsKeyboard({
    tabs: [{ id: "a" }, { id: "b" }],
    activeId: "a",
    stopId: "a",
    onSelect,
    listRef,
  });
  return (
    <div ref={listRef}>
      {["a", "b"].map((id) => (
        <button
          key={id}
          data-panel-tab={id}
          tabIndex={tabIndexFor(id)}
          onKeyDown={(event) => onTabKeyDown(id, event)}
        >
          {id}
        </button>
      ))}
    </div>
  );
}

function visibleLabels(): string[] {
  return Array.from(container.querySelectorAll<HTMLElement>(".nav-point-label")).map(
    (label) => label.textContent ?? "",
  );
}

beforeEach(() => {
  useAppStore.setState({
    installError: null,
    installing: null,
    plugins: { root: "C:/data/plugins", plugins: [], problem: null },
    refreshPlugins: vi.fn(async () => undefined),
  });
});

afterEach(async () => {
  wiring.stripChord = null;
  wiring.tabMove = null;
  wiring.closeTab = null;
  wiring.composerAction = null;
  wiring.crescentPage = null;
  if (root !== undefined) await act(async () => root.unmount());
  document.body.replaceChildren();
});

describe("the strip keyboard runs the keymap's matchers", () => {
  it("switches tabs only on the chord the matcher answers", async () => {
    const selectTab = vi.fn();
    await render(<StripHarness selectTab={selectTab} closeTab={vi.fn()} />);

    wiring.stripChord = () => "next";
    await press(window, OTHER_KEY);
    expect(selectTab).toHaveBeenCalledWith("b");

    selectTab.mockClear();
    wiring.stripChord = () => null;
    await press(window, "]", { altKey: true, shiftKey: true });
    expect(selectTab).not.toHaveBeenCalled();
  });

  it("walks the strip only on the move the matcher answers", async () => {
    const selectTab = vi.fn();
    await render(<StripHarness selectTab={selectTab} closeTab={vi.fn()} />);

    wiring.tabMove = () => "next";
    await press(container.querySelector("[data-chip='a']")!, OTHER_KEY);
    expect(selectTab).toHaveBeenCalledWith("b");

    selectTab.mockClear();
    wiring.tabMove = () => null;
    await press(container.querySelector("[data-chip='a']")!, "ArrowRight");
    expect(selectTab).not.toHaveBeenCalled();
  });

  it("closes the focused chip only when the matcher calls the key a close", async () => {
    const closeTab = vi.fn();
    await render(<StripHarness selectTab={vi.fn()} closeTab={closeTab} />);

    wiring.closeTab = () => true;
    await press(container.querySelector("[data-chip='a']")!, OTHER_KEY);
    expect(closeTab).toHaveBeenCalledWith("a");

    closeTab.mockClear();
    wiring.closeTab = () => false;
    await press(container.querySelector("[data-chip='a']")!, "Delete");
    expect(closeTab).not.toHaveBeenCalled();
  });
});

describe("the panel tabs keyboard runs the keymap's matcher", () => {
  it("moves only on the move the matcher names", async () => {
    const onSelect = vi.fn();
    await render(<PanelHarness onSelect={onSelect} />);

    wiring.tabMove = () => "next";
    await press(container.querySelector("[data-panel-tab='a']")!, OTHER_KEY);
    expect(onSelect).toHaveBeenCalledWith("b");

    onSelect.mockClear();
    wiring.tabMove = () => null;
    await press(container.querySelector("[data-panel-tab='a']")!, "ArrowRight");
    expect(onSelect).not.toHaveBeenCalled();
  });
});

describe("the composer runs the keymap's composer action", () => {
  it("submits only on the action the matcher answers", async () => {
    const onSend = vi.fn(async () => true);
    const mocks: ComposerMocks = { onSend, onQueue: vi.fn() };
    await render(<WorkspaceComposer {...composerProps(mocks)} />);
    const drive = composerDrivers(container);

    await drive.type("hello");
    wiring.composerAction = () => "submit";
    await drive.press("F2");
    expect(onSend).toHaveBeenCalledWith("hello", []);

    onSend.mockClear();
    await drive.type("again");
    wiring.composerAction = () => null;
    await drive.press("Enter");
    expect(onSend).not.toHaveBeenCalled();
  });
});

describe("the crescent runs the keymap's crescent matcher", () => {
  it("pages only on the page the matcher answers", async () => {
    await render(
      <Shell activeSurface="workspace">
        <div>Surface</div>
      </Shell>,
    );
    const sliver = container.querySelector<HTMLButtonElement>(".crescent-sliver");
    if (sliver === null) throw new Error("crescent sliver did not render");
    await act(async () => sliver.focus());
    expect(visibleLabels()[0]).toBe("Workspace");

    wiring.crescentPage = () => "next";
    await press(sliver, OTHER_KEY);
    expect(visibleLabels()[0]).toBe("Polis");

    wiring.crescentPage = () => null;
    await press(sliver, "ArrowRight");
    expect(visibleLabels()[0]).toBe("Polis");
  });
});
