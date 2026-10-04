// The page-side function that reads a document as prose, run against a fake
// DOM. It is the one function this app hands to a page, and nothing else here
// runs it, so its own bounds are proved here rather than by reading its source.
//
// The function is read out of the Rust file, so this test runs what ships.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

/** The function source, taken from the Rust constant that is written to CDP. */
function readable(): string {
  const source = readFileSync(join("src-tauri", "src", "browser", "commands", "read.rs"), "utf8");
  const block = source.match(/pub const READABLE: &str = r##"([\s\S]*?)"##;/);
  if (block === null) throw new Error("READABLE is not declared in read.rs");
  return block[1];
}

interface FakeNode {
  nodeType: number;
  tagName?: string;
  nodeValue?: string;
  hidden?: boolean;
  attributes: Record<string, string>;
  display: string;
  visibility: string;
  childNodes: FakeNode[];
  firstChild: FakeNode | null;
  lastChild: FakeNode | null;
  nextSibling: FakeNode | null;
  previousSibling: FakeNode | null;
  parent: FakeNode | null;
  getAttribute(name: string): string | null;
}

/** A node, with the sibling pointers the walk follows. */
function node(
  kind: "text" | "element",
  options: {
    tag?: string;
    value?: string;
    attrs?: Record<string, string>;
    hidden?: boolean;
    display?: string;
    visibility?: string;
    children?: FakeNode[];
  } = {},
): FakeNode {
  const made: FakeNode = {
    nodeType: kind === "text" ? 3 : 1,
    tagName: options.tag ?? "DIV",
    nodeValue: options.value ?? "",
    hidden: options.hidden ?? false,
    attributes: options.attrs ?? {},
    display: options.display ?? "block",
    visibility: options.visibility ?? "visible",
    childNodes: [],
    firstChild: null,
    lastChild: null,
    nextSibling: null,
    previousSibling: null,
    parent: null,
    getAttribute: (name) =>
      Object.prototype.hasOwnProperty.call(options.attrs ?? {}, name)
        ? (options.attrs ?? {})[name]
        : null,
  };
  for (const child of options.children ?? []) {
    child.parent = made;
    made.childNodes.push(child);
  }
  made.childNodes.forEach((child, index) => {
    child.nextSibling = made.childNodes[index + 1] ?? null;
    child.previousSibling = made.childNodes[index - 1] ?? null;
  });
  made.firstChild = made.childNodes[0] ?? null;
  made.lastChild = made.childNodes.at(-1) ?? null;
  return made;
}

const text = (value: string) => node("text", { value });

/** Run the function as the page would, and how many nodes it had to ask about. */
function run(body: FakeNode, budget: number): { answer: string; asked: number } {
  let asked = 0;
  const window = {
    getComputedStyle: (element: FakeNode) => {
      asked += 1;
      return { display: element.display, visibility: element.visibility };
    },
  };
  const document = { body };
  // The two names the page's own scope gives the function.
  const read = new Function("window", "document", `return (${readable()})`)(window, document);
  return { answer: read.call(body, budget), asked };
}

function read(body: FakeNode, budget: number): string {
  return run(body, budget).answer;
}

describe("the page-side reader", () => {
  it("reads a link as its own words and where it goes", () => {
    const body = node("element", {
      tag: "BODY",
      children: [
        node("element", {
          tag: "P",
          children: [
            text("See "),
            node("element", {
              tag: "A",
              attrs: { href: "/sign-in" },
              children: [text("sign in")],
            }),
            text(" first"),
          ],
        }),
      ],
    });

    expect(read(body, 12_000)).toBe("See [sign in](/sign-in) first");
  });

  it("folds a deep subtree under one link without overflowing the stack", () => {
    // A page can nest a link's own markup as deep as it likes. A reader that
    // walked it by recursion would throw on this before it ever counted a node.
    const link = (depth: number) => {
      let deepest = node("element", { tag: "SPAN", children: [text("deep")] });
      for (let level = 0; level < depth; level += 1) {
        deepest = node("element", { tag: "SPAN", children: [deepest] });
      }
      return node("element", {
        tag: "BODY",
        children: [node("element", { tag: "A", attrs: { href: "/deep" }, children: [deepest] })],
      });
    };

    // Nesting a page can really have: read whole.
    expect(read(link(500), 12_000)).toBe("[deep](/deep)");
    // Nesting it should not have: the answer is bounded and the call returns.
    const answer = read(link(50_000), 12_000);
    expect(answer.length).toBeLessThanOrEqual(12_000);
  });

  it("counts the nodes under a link towards the same budget as the rest", () => {
    // Forty thousand nodes of markup inside one link. The label is folded into
    // a single line, so without a node budget of its own this subtree is walked
    // in full — and the page's own node budget never sees any of it.
    const inside = Array.from({ length: 100_000 }, () =>
      node("element", { tag: "SPAN", children: [text("word")] }),
    );
    const body = node("element", {
      tag: "BODY",
      children: [
        node("element", {
          tag: "A",
          attrs: { href: "/fat" },
          children: inside,
        }),
      ],
    });

    // Every element the walk asks about is a node it visited, so the asks are
    // the size of the walk. Two asks per element inside `shown` and one for the
    // block check, so the walk cannot exceed the function's own node budget
    // whatever the page nests inside one link.
    const { asked } = run(body, 12_000);
    const budget = Number(/NODES = (\d+)/.exec(readable())?.[1] ?? 0);

    expect(budget).toBeGreaterThan(0);
    expect(asked).toBeLessThanOrEqual(budget * 2 + 10);
  });

  it("leaves a password field's contents unread", () => {
    const secret = "SENTINEL-PW-7f3a";
    const body = node("element", {
      tag: "BODY",
      children: [
        node("element", {
          tag: "FORM",
          children: [
            node("element", { tag: "INPUT", attrs: { type: "password" } }),
            // The mark a textarea carries. Its value IS its text, so this is
            // the path a passphrase would otherwise be read out through.
            node("element", {
              tag: "TEXTAREA",
              attrs: { autocomplete: "current-password" },
              children: [text(secret)],
            }),
            node("element", {
              tag: "INPUT",
              attrs: { type: "text", autocomplete: "current-password" },
            }),
            text("Sign in"),
          ],
        }),
      ],
    });

    expect(read(body, 12_000)).toBe("Sign in");
  });

  it("still reads an ordinary field that sits among them", () => {
    const body = node("element", {
      tag: "BODY",
      children: [
        node("element", {
          tag: "FORM",
          children: [
            node("element", { tag: "INPUT", attrs: { type: "password" } }),
            node("element", {
              tag: "TEXTAREA",
              attrs: { autocomplete: "note" },
              children: [text("the note a person wrote")],
            }),
          ],
        }),
      ],
    });

    expect(read(body, 12_000)).toBe("the note a person wrote");
  });
});
