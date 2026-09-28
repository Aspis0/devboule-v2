// The picker menu rows' look contract (SPEC-tokens type ramp): h28 rows, the
// name at 13 px, the meta line at 12 px, the UI font — nothing below 12 px in
// this sheet. The size cases read the real stylesheets through cssProof in
// both themes; the render cases prove long names and descriptions reach the
// rows they restyle, and that the menu scrolls its own box to the current row
// on open and to the focused row on arrow keys. The menu's side and cap
// (menuPlacement) are pinned in PickerChip.placement.test.tsx. happy-dom
// does no layout, so pixel fit is not asserted here — it is listed as
// unverified in CODER-REPORT-look-calls.md.
// @vitest-environment happy-dom
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { act, type ReactElement } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof } from "../features/workspace/cssProof";
import { PickerChip } from "./PickerChip";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../..");
const SHEETS = [
  readFileSync(resolve(rootDir, "src/styles/tokens.css"), "utf8"),
  readFileSync(resolve(rootDir, "src/components/PickerChip.css"), "utf8"),
];

const UI_FONT = 'font-family: "Inter", system-ui, sans-serif';

describe("the picker menu rows' sizes", () => {
  it.each(["light", "dark"] as const)("%s: the name is 13px, the meta line 12px", (theme) => {
    const css = assembleCssProof(SHEETS, theme);
    expect(css.rulesFor(".workspace-mode-name")).toContain("font-size: 13px");
    expect(css.rulesFor(".workspace-mode-description")).toContain("font-size: 12px");
  });

  it.each(["light", "dark"] as const)("%s: rows are h28 in the UI font", (theme) => {
    const css = assembleCssProof(SHEETS, theme);
    const rows = css.rulesFor(".workspace-mode-option");
    // A single-line row is exactly 28: 6 px of padding top and bottom plus
    // the name's 16 px line box; two-line rows grow past it.
    expect(rows).toContain("min-height: 28px");
    expect(rows).toContain("padding: 6px 9px");
    expect(css.rulesFor(".workspace-mode-name")).toContain("line-height: 16px");
    expect(rows).toContain(UI_FONT);
  });

  it("declares no size below 12px in the picker sheet", () => {
    const css = assembleCssProof(SHEETS);
    const small: string[] = [];
    for (const rule of css.rules) {
      for (const match of rule.body.matchAll(/font-size:\s*(\d+(?:\.\d+)?)px/g)) {
        if (Number.parseFloat(match[1]!) < 12) small.push(`${rule.selector}: ${match[0]}`);
      }
    }
    expect(small).toEqual([]);
  });

  it("lets rows keep their height so the menu scrolls instead of squashing them", () => {
    const css = assembleCssProof(SHEETS);
    // A flex column shrinks its items before it scrolls: without flex:none
    // the rows compress toward min-height and each meta line overprints the
    // next row's name. happy-dom does no layout, so the overprint itself is
    // unverified — see CODER-REPORT-look-calls.md.
    expect(css.rulesFor(".workspace-mode-option")).toContain("flex: none");
    expect(css.rulesFor(".workspace-mode-menu")).toContain("overflow-y: auto");
  });

  it("carries a fallback cap for opens the room maths cannot measure", () => {
    const css = assembleCssProof(SHEETS);
    // Every measurable open overwrites this inline (menuPlacement); the
    // sheet keeps 280 px so an unmeasurable open still scrolls inside a cap.
    expect(css.rulesFor(".workspace-mode-menu")).toContain("max-height: 280px");
  });

  it("declares a 320px ceiling and wraps long names instead of clipping them", () => {
    const css = assembleCssProof(SHEETS);
    // The ceiling never binds at current chip widths (the menu shrink-to-fits
    // near min-width): pinned as a declaration, not as the measured width.
    expect(css.rulesFor(".workspace-mode-menu")).toContain("max-width: 320px");
    expect(css.rulesFor(".workspace-mode-menu")).toContain("min-width: 220px");
    // No nowrap on the option or its lines: a long model name wraps inside
    // the menu rather than painting past it.
    for (const selector of [".workspace-mode-option", ".workspace-mode-name"]) {
      expect(css.rulesFor(selector)).not.toContain("white-space: nowrap");
    }
  });
});

const SCROLL_OPTIONS = [
  { id: "m1", name: "First", description: "First description" },
  { id: "m2", name: "Second", description: "Second description" },
  { id: "m3", name: "Third", description: "Third description" },
];

/** The geometry happy-dom does not lay out: rows at content offsets, a box
 * that shows a window of them. Values the scroll maths reads, nothing more. */
function stubRowGeometry(row: Element, offsetTop: number, offsetHeight: number): void {
  Object.defineProperty(row, "offsetTop", { value: offsetTop, configurable: true });
  Object.defineProperty(row, "offsetHeight", { value: offsetHeight, configurable: true });
}

async function openScrollMenu(currentId: string): Promise<{
  root: Root;
  menu: HTMLElement;
  rows: HTMLButtonElement[];
  show: (nextId: string) => Promise<void>;
}> {
  const chip = (selected: string): ReactElement => (
    <PickerChip
      label="Model"
      options={SCROLL_OPTIONS}
      currentId={selected}
      onSelect={() => undefined}
      chipTestId="model-chip"
      optionTestId={(id) => `model-option-${id}`}
    />
  );
  // Inside the clipping panel, so the room maths takes the panel branch.
  const container = document.createElement("div");
  container.className = "workspace-center-panel";
  document.body.appendChild(container);
  const root = createRoot(container);
  await act(async () => {
    root.render(chip(currentId));
  });
  const trigger = container.querySelector<HTMLButtonElement>('[data-testid="model-chip"]');
  if (trigger === null) throw new Error("picker trigger did not render");
  await act(async () => trigger.click());
  const menu = container.querySelector<HTMLElement>(".workspace-mode-menu");
  if (menu === null) throw new Error("picker menu did not open");
  return {
    root,
    menu,
    rows: [...menu.querySelectorAll<HTMLButtonElement>("[role='option']")],
    show: async (nextId: string) => {
      await act(async () => {
        root.render(chip(nextId));
      });
    },
  };
}

describe("the picker menu's own scroll", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("brings the current row into view when the selection changes under the open menu", async () => {
    const { root, menu, rows, show } = await openScrollMenu("m1");
    let scrollTop = 0;
    stubRowGeometry(rows[0], 0, 48);
    stubRowGeometry(rows[1], 48, 48);
    stubRowGeometry(rows[2], 96, 48);
    Object.defineProperty(menu, "clientHeight", { value: 80, configurable: true });
    Object.defineProperty(menu, "scrollTop", {
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
      configurable: true,
    });

    // The third row runs 96–144 through an 80 px window: the menu scrolls 64.
    await show("m3");
    expect(scrollTop).toBe(64);

    await act(async () => root.unmount());
  });

  it("scrolls the focused row into view on arrow keys, inside the menu only", async () => {
    const { root, menu, rows } = await openScrollMenu("m1");
    let scrollTop = 0;
    stubRowGeometry(rows[0], 0, 48);
    stubRowGeometry(rows[1], 48, 48);
    stubRowGeometry(rows[2], 96, 48);
    Object.defineProperty(menu, "clientHeight", { value: 80, configurable: true });
    Object.defineProperty(menu, "scrollTop", {
      get: () => scrollTop,
      set: (value: number) => {
        scrollTop = value;
      },
      configurable: true,
    });
    const press = async (key: "ArrowDown" | "ArrowUp"): Promise<void> => {
      await act(async () => {
        menu.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }));
      });
    };

    await press("ArrowDown");
    expect(document.activeElement).toBe(rows[0]);
    expect(scrollTop).toBe(0);
    await press("ArrowDown");
    expect(document.activeElement).toBe(rows[1]);
    // The second row runs 48–96 through the 80 px window: the menu scrolls 16.
    expect(scrollTop).toBe(16);
    await press("ArrowDown");
    expect(document.activeElement).toBe(rows[2]);
    expect(scrollTop).toBe(64);
    await press("ArrowUp");
    expect(document.activeElement).toBe(rows[1]);
    expect(scrollTop).toBe(48);

    await act(async () => root.unmount());
  });
});

describe("the picker menu rows' content", () => {
  afterEach(() => {
    document.body.innerHTML = "";
  });

  it("shows long names and descriptions in the open menu", async () => {
    const longName = "qwen3.5-4b-instruct-2507-extra-long-variant-name-for-width";
    const container = document.createElement("div");
    document.body.appendChild(container);
    const root = createRoot(container);
    await act(async () => {
      root.render(
        <PickerChip
          label="Model"
          options={[
            {
              id: "m1",
              name: longName,
              description: "The default everyday model · 200,000 tokens",
            },
            { id: "high", name: "High", description: "Reasons longer before answering" },
          ]}
          currentId="m1"
          onSelect={() => undefined}
          chipTestId="model-chip"
          optionTestId={(id) => `model-option-${id}`}
        />,
      );
    });
    const trigger = container.querySelector<HTMLButtonElement>('[data-testid="model-chip"]');
    if (trigger === null) throw new Error("picker trigger did not render");
    await act(async () => trigger.click());

    const menu = container.querySelector(".workspace-mode-menu");
    if (menu === null) throw new Error("picker menu did not open");
    expect(menu.querySelector(".workspace-mode-name")?.textContent).toBe(longName);
    expect(menu.querySelector(".workspace-mode-description")?.textContent).toContain(
      "200,000 tokens",
    );
    expect(menu.textContent).toContain("Reasons longer before answering");

    await act(async () => root.unmount());
  });
});
