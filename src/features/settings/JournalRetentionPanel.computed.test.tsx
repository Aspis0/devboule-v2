// @vitest-environment happy-dom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { assembleCssProof, removeCssProof } from "../workspace/cssProof";

vi.mock("../../lib/tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../lib/tauri")>();
  const retention = {
    sessionMaxBytes: { value: 1024, source: "default" },
    maxBytes: { value: 2048, source: "default" },
    maxSessions: { value: 30, source: "default" },
    maxAgeMs: { value: 0, source: "default" },
  };
  return {
    ...actual,
    journalRetentionGet: vi.fn(async () => retention),
    journalRetentionSet: vi.fn(async () => retention),
    journalUsage: vi.fn(async () => ({
      totalBytes: 0,
      sessionCount: 0,
      deletedByUser: 0,
      deletedByRetention: 0,
      unreclaimable: { bytesOver: 0, sessionsOver: 0, agedOut: 0 },
      limits: {
        snapshotEveryBytes: 1024,
        sessionMaxBytes: 1024,
        maxBytes: 2048,
        maxSessions: 30,
        maxAgeMs: 0,
      },
      perSession: [],
    })),
  };
});

import { JournalRetentionPanel } from "./JournalRetentionPanel";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const rootDir = resolve(import.meta.dirname, "../../..");
const bundlePaths = [
  "src/styles/tokens.css",
  "src/styles/global.css",
  "src/features/settings/diagnostics.css",
  "src/features/settings/devices.css",
  "src/features/oracle/oracle.css",
  "src/features/settings/providers.css",
  "src/features/settings/profiles.css",
  "src/features/settings/projects.css",
  "src/features/settings/settings.css",
] as const;
const sheets = bundlePaths.map((path) => readFileSync(resolve(rootDir, path), "utf8"));
const proof = assembleCssProof(sheets);

function readTokens(selector: string): Map<string, string> {
  const css = sheets[0]!.replace(/\/\*[\s\S]*?\*\//g, "");
  const map = new Map<string, string>();
  const escaped = selector.replace(/[^a-z0-9]/gi, "\\$&");
  const block = new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, "g");
  for (const match of css.matchAll(block)) {
    for (const decl of match[1]!.matchAll(/--([\w-]+):\s*([^;]+);/g)) {
      map.set(`--${decl[1]}`, decl[2]!.trim());
    }
  }
  return map;
}

function resolveColor(value: string, root: Map<string, string>, dark: Map<string, string>): string {
  const variable = value.match(/^var\((--[\w-]+)\)$/)?.[1];
  if (variable === undefined) {
    if (/^#[\da-f]{3}$/i.test(value)) {
      return `#${[...value.slice(1)].map((channel) => channel + channel).join("")}`;
    }
    if (/^#[\da-f]{6}$/i.test(value)) return value.toLowerCase();
    throw new Error(`unsupported literal color ${value}`);
  }
  let color = dark.get(variable) ?? root.get(variable);
  for (let pass = 0; pass < 4 && color?.startsWith("var("); pass += 1) {
    const name = color.match(/^var\((--[\w-]+)\)$/)?.[1];
    color = name === undefined ? color : (dark.get(name) ?? root.get(name));
  }
  if (color === undefined || !/^#[\da-f]{6}$/i.test(color)) {
    throw new Error(`could not resolve ${value} to a theme color`);
  }
  return color;
}

function luminance(hex: string): number {
  const channels = [1, 3, 5].map((at) => {
    const channel = Number.parseInt(hex.slice(at, at + 2), 16) / 255;
    return channel <= 0.03928 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * channels[0]! + 0.7152 * channels[1]! + 0.0722 * channels[2]!;
}

function contrastRatio(a: string, b: string): number {
  const [hi, lo] = luminance(a) > luminance(b) ? [a, b] : [b, a];
  return (luminance(hi) + 0.05) / (luminance(lo) + 0.05);
}

interface CascadeRule {
  selector: string;
  body: string;
  order: number;
}

function parseRules(css: string): CascadeRule[] {
  const rules: CascadeRule[] = [];
  let order = 0;
  const walk = (source: string): void => {
    let index = 0;
    while (index < source.length) {
      const open = source.indexOf("{", index);
      if (open < 0) break;
      const selector = source.slice(index, open).trim();
      let depth = 1;
      let cursor = open + 1;
      while (depth > 0 && cursor < source.length) {
        if (source[cursor] === "{") depth += 1;
        if (source[cursor] === "}") depth -= 1;
        cursor += 1;
      }
      const body = source.slice(open + 1, cursor - 1);
      if (selector.startsWith("@media")) {
        const query = selector.slice("@media".length).trim();
        if (typeof window.matchMedia !== "function" || window.matchMedia(query).matches) walk(body);
      } else if (!selector.startsWith("@")) {
        for (const part of selector.split(",")) {
          rules.push({ selector: part.trim(), body, order: order++ });
        }
      }
      index = cursor;
    }
  };
  walk(css.replace(/\/\*[\s\S]*?\*\//g, ""));
  return rules;
}

function matchingSelectors(target: Element, css: string): string[] {
  return [
    ...new Set(
      parseRules(css).flatMap(({ selector }) => {
        try {
          return target.matches(selector) ? [selector] : [];
        } catch {
          return [];
        }
      }),
    ),
  ];
}

function winningValue(
  target: Element,
  property: "background" | "border-color",
  css: string,
): { value: string; important: boolean } {
  const candidates = parseRules(css).flatMap(({ selector, body, order }) => {
    let applies = false;
    try {
      applies = target.matches(selector);
    } catch {
      return [];
    }
    if (!applies) return [];
    return body.split(";").flatMap((raw, declarationOrder) => {
      const colon = raw.indexOf(":");
      if (colon < 0) return [];
      const name = raw.slice(0, colon).trim().toLowerCase();
      const matchesProperty =
        property === "background"
          ? name === "background" || name === "background-color"
          : name === "border" || name === "border-color";
      if (!matchesProperty) return [];
      const rawValue = raw.slice(colon + 1).trim();
      const important = /!important\s*$/i.test(rawValue);
      const value = rawValue.replace(/\s*!important\s*$/i, "").trim();
      const color = value.match(/var\(--[\w-]+\)|#[\da-f]{3,8}\b/i)?.[0];
      if (color === undefined) throw new Error(`cannot resolve ${name}: ${value}`);
      return [
        {
          selector,
          value: color,
          important,
          order: order * 100 + declarationOrder,
        },
      ];
    });
  });
  const specificity = (selector: string): [number, number, number] => [
    (selector.match(/#[\w-]+/g) ?? []).length,
    (selector.match(/\.[\w-]+|\[[^\]]+\]|:[\w-]+/g) ?? []).length,
    (selector.match(/(?:^|[\s>+~])(?:[a-z][\w-]*)/gi) ?? []).length,
  ];
  candidates.sort((a, b) => {
    const left = specificity(a.selector);
    const right = specificity(b.selector);
    return (
      Number(a.important) - Number(b.important) ||
      left[0] - right[0] ||
      left[1] - right[1] ||
      left[2] - right[2] ||
      a.order - b.order
    );
  });
  const winner = candidates.at(-1);
  if (winner === undefined) throw new Error(`no winning ${property} declaration for mounted input`);
  return { value: winner.value, important: winner.important };
}

afterEach(() => {
  removeCssProof();
  document.body.innerHTML = "";
});

describe("journal retention input cascade", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(async () => {
    if (root) await act(async () => root.unmount());
    container.remove();
  });

  it("separates the real mounted input from its retention card", async () => {
    root = createRoot(container);
    await act(async () => {
      root.render(
        <section className="surface-card settings-surface">
          <div className="settings-main-inner">
            {/* SettingsSurface mounts the diagnostics panel and retention panel as siblings. */}
            <div id="settings-panel-diagnostics" />
            <JournalRetentionPanel />
          </div>
        </section>,
      );
    });
    await act(async () => undefined);

    const input = container.querySelector<HTMLInputElement>(".retention-limit-input");
    const card = container.querySelector<HTMLElement>(".retention-limits");
    if (!input || !card) throw new Error("real retention input and card should render");
    expect(input.closest("#settings-panel-diagnostics")).toBeNull();
    const css = sheets.join("\n");
    const background = winningValue(input, "background", css);
    const border = winningValue(input, "border-color", css);
    proof.inject([...matchingSelectors(input, css), ...matchingSelectors(card, css)]);
    const inputStyle = getComputedStyle(input);
    const cardStyle = getComputedStyle(card);
    const tokens = readTokens(":root");
    const darkTokens = readTokens('[data-theme="dark"]');
    const lightBackground = resolveColor(background.value, tokens, tokens);
    const lightBorder = resolveColor(border.value, tokens, tokens);
    expect(inputStyle.backgroundColor).toBe(lightBackground);
    expect(inputStyle.borderColor).toBe(lightBorder);
    expect(inputStyle.backgroundColor).not.toBe(cardStyle.backgroundColor);
    expect(background.value).not.toBe("var(--panel-card)");
    expect(border.value).toBe("var(--line-strong)");

    const themes = [tokens, new Map([...tokens, ...darkTokens])];
    for (const theme of themes) {
      const cardColor = resolveColor("var(--panel-card)", tokens, theme);
      expect(
        contrastRatio(resolveColor(background.value, tokens, theme), cardColor),
      ).toBeGreaterThanOrEqual(1.05);
      expect(
        contrastRatio(resolveColor(border.value, tokens, theme), cardColor),
      ).toBeGreaterThanOrEqual(1.3);
    }
  });

  it("ranks literal colors and important declarations in the actual cascade", () => {
    const wrapper = document.createElement("div");
    wrapper.id = "retention-cascade-test";
    wrapper.className = "retention-panel";
    const input = document.createElement("input");
    input.className = "retention-limit-input";
    wrapper.appendChild(input);
    document.body.appendChild(wrapper);
    const css = [
      "#retention-cascade-test .retention-limit-input { background: var(--panel-card); }",
      ".retention-panel .retention-limit-input { background: var(--panel-side) !important; }",
      ".retention-panel .retention-limit-input { background: #000000 !important; }",
    ].join("\n");
    expect(winningValue(input, "background", css)).toEqual({
      value: "#000000",
      important: true,
    });
  });
});
