// Selects inherit the global reset: Inter from the body and the primary ink
// for the value text. The design picker keeps its own author size; the
// settings selects share one interface-size rule with the input beside them,
// and dim while disabled instead of keeping the reset's primary ink. The
// History host filter keeps the primary ink in its own page-bar wrapper.
// @vitest-environment happy-dom
import { readdirSync, readFileSync } from "node:fs";
import { join, relative, resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../features/workspace/cssProof";

const ROOT = resolve(import.meta.dirname, "../..");
const read = (rel: string) => readFileSync(resolve(ROOT, rel), "utf8");

const SHEETS = [
  "src/styles/tokens.css",
  "src/styles/global.css",
  "src/features/design/design.css",
  "src/features/settings/settings.css",
  "src/features/history/history.css",
];

afterEach(removeCssProof);

// Lexical JSX nesting per file: a file-level substring cannot see a select
// moved out of its wrapper while the wrapper string survives elsewhere.
function selectsIn(source: string): { line: number; wrapped: boolean }[] {
  const frames: { name: string | null; wrapper: boolean }[] = [];
  const selects: { line: number; wrapped: boolean }[] = [];
  const VOID = new Set(["input", "br", "hr", "img", "link", "meta"]);
  let i = 0;
  while (i < source.length) {
    if (source[i] !== "<" || !/[A-Za-z/>]/.test(source[i + 1] ?? "")) {
      i += 1;
      continue;
    }
    let j = i + 1;
    let brace = 0;
    let quote: string | null = null;
    while (j < source.length) {
      const char = source[j]!;
      if (quote !== null) {
        if (char === quote) quote = null;
      } else if (char === '"' || char === "'" || char === "`") quote = char;
      else if (char === "{") brace += 1;
      else if (char === "}") brace = Math.max(0, brace - 1);
      else if (char === ">" && brace === 0) break;
      j += 1;
    }
    if (j >= source.length) break;
    const tag = source.slice(i, j + 1);
    i = j + 1;
    const closing = /^<\/\s*([A-Za-z][\w.]*)?/.exec(tag);
    if (closing !== null) {
      const name = closing[1] ?? null;
      if (name === null) frames.pop();
      else {
        const at = frames.map((frame) => frame.name).lastIndexOf(name);
        if (at >= 0) frames.splice(at);
      }
      continue;
    }
    const opening = /^<\s*([A-Za-z][\w.]*)/.exec(tag);
    const name = opening === null ? null : opening[1]!;
    if (name === "select") {
      selects.push({
        line: source.slice(0, i).split("\n").length,
        wrapped: frames.some((frame) => frame.wrapper),
      });
    }
    if (/\/\s*>$/.test(tag) || name === null || VOID.has(name)) continue;
    const classAttr = /className\s*=\s*("(?:[^"]*)"|'(?:[^']*)'|`(?:[^`]*)`|\{[\s\S]*?\})/.exec(
      tag,
    );
    frames.push({
      name,
      wrapper:
        classAttr !== null &&
        (classAttr[1]!.includes("device-field") ||
          classAttr[1]!.includes("design-agent-picker") ||
          classAttr[1]!.includes("history-page-host")),
    });
  }
  return selects;
}

function buildDesignSelect(label: string): HTMLSelectElement {
  const picker = document.createElement("div");
  picker.className = "design-agent-picker";
  const select = document.createElement("select");
  select.setAttribute("aria-label", label);
  picker.appendChild(select);
  document.body.appendChild(picker);
  return select;
}

function buildSettingsSelect(label: string): HTMLSelectElement {
  const field = document.createElement("label");
  field.className = "device-field";
  field.textContent = label;
  const select = document.createElement("select");
  select.setAttribute("aria-label", label);
  field.appendChild(select);
  document.body.appendChild(field);
  return select;
}

describe("design selects keep their author size on the reset", () => {
  it.each(["light", "dark"] as const)("model and effort read Inter 13px (%s)", (theme) => {
    const css = assembleCssProof(SHEETS.map(read), theme);
    css.inject(["body", "select", ".design-agent-picker select"]);
    const model = buildDesignSelect("Model");
    const effort = buildDesignSelect("Thinking effort");
    for (const select of [model, effort]) {
      const style = getComputedStyle(select);
      expect(style.fontFamily).toContain("Inter");
      expect(style.fontSize).toBe("13px");
    }
    model.parentElement!.remove();
    effort.parentElement!.remove();
  });

  it("the author size overrides the reset's inherit", () => {
    const css = assembleCssProof(SHEETS.map(read));
    expect(css.rulesFor("select")).toContain("font: inherit");
    expect(css.rulesFor(".design-agent-picker select")).toContain("font-size: 13px");
  });
});

describe("settings field controls share one size", () => {
  it.each(["light", "dark"] as const)(
    "input and select read the same interface size (%s)",
    (theme) => {
      const css = assembleCssProof(SHEETS.map(read), theme);
      css.inject([
        "body",
        ".device-field",
        ".device-field input",
        ".device-field select",
        "select",
      ]);
      const field = document.createElement("label");
      field.className = "device-field";
      const input = document.createElement("input");
      const select = document.createElement("select");
      select.setAttribute("aria-label", "Provider");
      field.append(input, select);
      document.body.appendChild(field);
      expect(getComputedStyle(input).fontSize).toBe("14px");
      expect(getComputedStyle(select).fontSize).toBe("14px");
      expect(getComputedStyle(select).fontSize).toBe(getComputedStyle(input).fontSize);
      field.remove();
    },
  );
});

describe("selects paint the primary ink in both themes", () => {
  it.each(["light", "dark"] as const)("all seven selects read --ink (%s)", (theme) => {
    const css = assembleCssProof(SHEETS.map(read), theme);
    css.inject([
      "body",
      "select",
      ".design-agent-picker select",
      ".device-field",
      ".history-page-host select",
    ]);
    const expected = css.token("--ink")!;
    const hostWrap = document.createElement("label");
    hostWrap.className = "history-page-host";
    const host = document.createElement("select");
    host.setAttribute("aria-label", "Host");
    hostWrap.appendChild(host);
    document.body.appendChild(hostWrap);
    const selects = [
      buildDesignSelect("Model"),
      buildDesignSelect("Thinking effort"),
      buildSettingsSelect("Profile feature"),
      buildSettingsSelect("Provider"),
      buildSettingsSelect("Vocabulary"),
      host,
      buildSettingsSelect("Effort"),
      buildSettingsSelect("Waiting"),
    ];
    for (const select of selects) {
      expect(getComputedStyle(select).color).toBe(expected);
    }
    for (const select of selects) select.parentElement!.remove();
  });
});

describe("a busy settings field dims instead of keeping the enabled ink", () => {
  it.each(["light", "dark"] as const)(
    "disabled input and select read the dimmed treatment (%s)",
    (theme) => {
      const css = assembleCssProof(SHEETS.map(read), theme);
      css.inject([
        "body",
        ".device-field",
        ".device-field input",
        ".device-field select",
        ".device-field input:disabled",
        ".device-field select:disabled",
        "select",
      ]);
      // The dimmed colour is the muted ramp value, not a new hex: the rule
      // source carries it resolved, and --silence aliases --muted.
      expect(css.rulesFor(".device-field input:disabled")).toContain(css.token("--muted")!);
      expect(css.rulesFor(".device-field select:disabled")).toContain(css.token("--muted")!);
      const field = document.createElement("label");
      field.className = "device-field";
      const input = document.createElement("input");
      input.disabled = true;
      const select = document.createElement("select");
      select.setAttribute("aria-label", "Provider");
      select.disabled = true;
      field.append(input, select);
      document.body.appendChild(field);
      for (const control of [input, select]) {
        const style = getComputedStyle(control);
        expect(style.color).toBe(css.token("--muted"));
        expect(style.color).not.toBe(css.token("--ink"));
        expect(style.opacity).toBe("0.55");
      }
      field.remove();
    },
  );
});

describe("every real select lives in a wrapped field", () => {
  it("each <select> in src nests inside .device-field, .design-agent-picker or .history-page-host", () => {
    const sources: string[] = [];
    const walk = (dir: string): void => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = join(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (entry.name.endsWith(".tsx") && !entry.name.endsWith(".test.tsx"))
          sources.push(full);
      }
    };
    walk(resolve(ROOT, "src"));
    const found: string[] = [];
    const unwrapped: string[] = [];
    for (const file of sources) {
      for (const select of selectsIn(readFileSync(file, "utf8"))) {
        const where = `${relative(ROOT, file)}:${select.line}`;
        found.push(where);
        if (!select.wrapped) unwrapped.push(where);
      }
    }
    expect(unwrapped).toEqual([]);
    expect(found).toHaveLength(8);
  });
});
