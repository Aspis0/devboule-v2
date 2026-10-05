// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { assembleCssProof, removeCssProof } from "../cssProof";

const rootDir = resolve(import.meta.dirname, "../../../..");
const read = (path: string) => readFileSync(resolve(rootDir, path), "utf8");
const railSheets = () => [
  read("src/styles/tokens.css"),
  read("src/features/workspace/Workspace.css"),
  read("src/features/workspace/timeline/TurnRail.css"),
];
const railCss = assembleCssProof(railSheets());
const railCssDark = assembleCssProof(railSheets(), "dark");

afterEach(removeCssProof);

describe("turn rail computed styles", () => {
  it("keeps the conversation's own padding and reserves the gutter in the content box", () => {
    railCss.inject([
      ".workspace-conversation",
      ".workspace-conversation.has-turn-rail",
      ".workspace-conversation.has-turn-rail .workspace-conversation-content",
    ]);
    const conversation = document.createElement("div");
    conversation.className = "workspace-conversation";
    const content = document.createElement("div");
    content.className = "workspace-conversation-content";
    conversation.appendChild(content);
    document.body.appendChild(conversation);

    expect(getComputedStyle(conversation).paddingLeft).toBe("16px");

    conversation.classList.add("has-turn-rail");
    // The conversation's own box does not change with the rail; the
    // content box gives up 32 px of column — 16 + 32 = 48 from the edge.
    expect(getComputedStyle(conversation).paddingLeft).toBe("16px");
    expect(getComputedStyle(content).paddingLeft).toBe("32px");
    expect(getComputedStyle(conversation).position).toBe("relative");
    expect(getComputedStyle(content).position).toBe("relative");
    conversation.remove();
  });

  it("insets the permission card and the composer with the rail and leaves the queue track on the composer card", () => {
    railCss.inject([
      ".workspace-conversation",
      ".workspace-conversation.has-turn-rail > .permission-card",
      ".workspace-composer-wrap",
      ".workspace-composer-track",
      ".workspace-agent-shell.has-turn-rail .workspace-composer-track",
      ".workspace-composer",
      ".workspace-agent-shell.has-turn-rail .workspace-composer",
    ]);
    const build = (
      withRail: boolean,
    ): {
      shell: HTMLDivElement;
      aux: HTMLDivElement;
      track: HTMLDivElement;
      composer: HTMLDivElement;
    } => {
      const shell = document.createElement("div");
      shell.className = withRail ? "workspace-agent-shell has-turn-rail" : "workspace-agent-shell";
      const conversation = document.createElement("div");
      conversation.className = withRail
        ? "workspace-conversation has-turn-rail"
        : "workspace-conversation";
      const content = document.createElement("div");
      content.className = "workspace-conversation-content";
      const aux = document.createElement("div");
      aux.className = "permission-card";
      aux.setAttribute("data-testid", "aux-node");
      conversation.appendChild(content);
      conversation.appendChild(aux);
      const wrap = document.createElement("div");
      wrap.className = "workspace-composer-wrap";
      const track = document.createElement("div");
      track.className = "workspace-composer-track";
      const composer = document.createElement("div");
      composer.className = "workspace-composer";
      wrap.appendChild(track);
      wrap.appendChild(composer);
      shell.appendChild(conversation);
      shell.appendChild(wrap);
      document.body.appendChild(shell);
      return { shell, aux, track, composer };
    };

    // Rail off: no sibling takes an inset; the composer keeps its 12 and the
    // track shares the lane rule, so it keeps the composer's box.
    const off = build(false);
    expect(getComputedStyle(off.aux).marginLeft).toBe("");
    expect(getComputedStyle(off.track).paddingLeft).toBe("");
    expect(getComputedStyle(off.track).maxWidth).toBe("calc(100% - 32px)");
    expect(getComputedStyle(off.composer).paddingLeft).toBe("12px");
    off.shell.remove();

    // The card takes the gutter as margin, the composer card as padding; the
    // track takes no rail inset and keeps the card's box in both states.
    // Separate trees per state with classes set at build: happy-dom keeps a
    // stale computed style for deeper descendants after a later class add.
    const on = build(true);
    expect(getComputedStyle(on.aux).marginLeft).toBe("32px");
    expect(getComputedStyle(on.track).paddingLeft).toBe("");
    expect(getComputedStyle(on.track).maxWidth).toBe("calc(100% - 32px)");
    expect(getComputedStyle(on.composer).paddingLeft).toBe("48px");
    on.shell.remove();
  });

  it("keeps the conversation inside a 720 px pane with the rail on and off", () => {
    railCss.inject([
      ".workspace-conversation",
      ".workspace-conversation.has-turn-rail",
      ".workspace-conversation.has-turn-rail .workspace-conversation-content",
      ".turn-rail",
    ]);
    const build = (withRail: boolean): HTMLDivElement => {
      const pane = document.createElement("div");
      pane.className = "workspace-agent-shell";
      pane.style.width = "720px";
      const conversation = document.createElement("div");
      conversation.className = withRail
        ? "workspace-conversation has-turn-rail"
        : "workspace-conversation";
      const content = document.createElement("div");
      content.className = "workspace-conversation-content";
      const row = document.createElement("div");
      // A long unbreakable row: it scrolls itself, never the conversation.
      row.style.whiteSpace = "pre";
      row.style.overflowX = "auto";
      row.textContent = "y".repeat(900);
      content.appendChild(row);
      conversation.appendChild(content);
      pane.appendChild(conversation);
      document.body.appendChild(pane);
      return conversation;
    };
    const off = build(false);
    const on = build(true);

    // The pane cap: width 760 bound by max-width calc(100% - 32px) of the
    // 720 px parent — the conversation is bounded by the pane and the chat
    // cap in both states, inset like the composer card.
    expect(getComputedStyle(off).maxWidth).toBe("calc(100% - 32px)");
    expect(getComputedStyle(on).maxWidth).toBe("calc(100% - 32px)");
    // The gutter comes out of the column: the conversation's own padding
    // is the same with the rail on and off.
    expect(getComputedStyle(off).paddingLeft).toBe("16px");
    expect(getComputedStyle(on).paddingLeft).toBe("16px");
    expect(getComputedStyle(on.querySelector(".workspace-conversation-content")!).paddingLeft).toBe(
      "32px",
    );

    // Never horizontally scrollable, by the same cap plus the row
    // containing itself. happy-dom stores scrollWidth/clientWidth at 0
    // (no layout engine — happy-dom's Element.js), so these three lines
    // hold the invariant in the test's terms; the numbers are the live
    // check's to measure.
    for (const conversation of [off, on]) {
      expect(conversation.scrollWidth).toBeLessThanOrEqual(conversation.clientWidth);
      expect(conversation.scrollWidth).toBeLessThanOrEqual(720);
      expect(conversation.scrollWidth).toBeLessThanOrEqual(760);
    }
    off.parentElement!.remove();
    on.parentElement!.remove();
  });

  it("threads the gutter at 1 px of line and parks the rail in it", () => {
    railCss.inject([".turn-rail", ".turn-rail-thread", ".turn-rail-stop"]);
    const rail = document.createElement("div");
    rail.className = "turn-rail";
    const thread = document.createElement("span");
    thread.className = "turn-rail-thread";
    const stop = document.createElement("span");
    stop.className = "turn-rail-stop";
    rail.appendChild(thread);
    rail.appendChild(stop);
    document.body.appendChild(rail);

    const railStyle = getComputedStyle(rail);
    expect(railStyle.position).toBe("absolute");
    expect(railStyle.width).toBe("56px");
    // The content box starts 16 px in (the conversation's padding), so
    // -16 reaches the conversation's left edge and the thread its 16 px.
    expect(railStyle.left).toBe("-16px");
    expect(railStyle.pointerEvents).toBe("none");
    const threadStyle = getComputedStyle(thread);
    expect(threadStyle.width).toBe("1px");
    expect(threadStyle.backgroundColor).toBe("#ded6c4");
    expect(getComputedStyle(stop).position).toBe("absolute");
    rail.remove();
  });

  it("rests the dot at 5 px idle, grows it on hover, and rings the current one at 9", () => {
    railCss.inject([
      ".turn-rail-glyph",
      ".turn-rail-dot:hover .turn-rail-glyph",
      '.turn-rail-dot[aria-current="true"] .turn-rail-glyph',
    ]);
    const idle = document.createElement("span");
    idle.className = "turn-rail-glyph";
    document.body.appendChild(idle);
    const idleStyle = getComputedStyle(idle);
    expect(idleStyle.width).toBe("5px");
    expect(idleStyle.height).toBe("5px");
    // The resting mark is a control's only visual: the resolved text-safe
    // mix — never the raw idle tone. (happy-dom returns "" for color-mix,
    // so the pin below reads the assembled declaration, not the engine.
    // Colors resolve from the tokens like the contrast suite does; only the
    // mix percentages are literal.)
    expect(railCss.rulesFor(".turn-rail-glyph")).toContain(
      `color-mix(in srgb, ${railCss.token("--tone-idle")!} 40%, ${railCss.token("--ink")!})`,
    );
    idle.remove();

    const dot = document.createElement("button");
    dot.className = "turn-rail-dot";
    dot.setAttribute("aria-current", "true");
    const glyph = document.createElement("span");
    glyph.className = "turn-rail-glyph";
    dot.appendChild(glyph);
    document.body.appendChild(dot);
    const currentStyle = getComputedStyle(glyph);
    expect(currentStyle.width).toBe("9px");
    expect(currentStyle.height).toBe("9px");
    expect(currentStyle.backgroundColor).toBe("#bd4a26");
    expect(currentStyle.boxShadow).toContain("3px");
    dot.remove();

    // Hover grows a resting dot to 8; the current rule comes after it in
    // the sheet, so a hovered current dot keeps its 9.
    expect(railCss.rulesFor(".turn-rail-dot:hover .turn-rail-glyph")).toContain("width: 8px");
    const selectors = railCss.rules.map((rule) => rule.selector);
    expect(selectors.indexOf(".turn-rail-dot:hover .turn-rail-glyph")).toBeLessThan(
      selectors.indexOf('.turn-rail-dot[aria-current="true"] .turn-rail-glyph'),
    );
  });

  it("opens a 224 px card beside the dot with a 12 px title and time", () => {
    railCss.inject([".turn-rail-preview", ".turn-rail-preview-title", ".turn-rail-preview-time"]);
    const preview = document.createElement("span");
    preview.className = "turn-rail-preview";
    const title = document.createElement("span");
    title.className = "turn-rail-preview-title";
    const time = document.createElement("span");
    time.className = "turn-rail-preview-time";
    preview.appendChild(title);
    preview.appendChild(time);
    document.body.appendChild(preview);

    const previewStyle = getComputedStyle(preview);
    expect(previewStyle.position).toBe("absolute");
    expect(previewStyle.width).toBe("224px");
    expect(previewStyle.borderRadius).toBe("8px");
    expect(previewStyle.backgroundColor).toBe("#fbf8f1");
    expect(previewStyle.display).toBe("none");
    // Left, not the UA button centre the card would otherwise inherit.
    expect(previewStyle.textAlign).toBe("left");
    const titleStyle = getComputedStyle(title);
    expect(titleStyle.fontSize).toBe("12px");
    expect(titleStyle.color).toBe("#1c1a17");
    expect(titleStyle.textOverflow).toBe("ellipsis");
    // The time sits under the title: the spec sizes it 12 like the title,
    // and small transcript metadata is muted.
    const timeStyle = getComputedStyle(time);
    expect(timeStyle.display).toBe("block");
    expect(timeStyle.fontSize).toBe("12px");
    expect(timeStyle.color).toBe(railCss.token("--muted"));
    preview.remove();
  });

  it("shows the card only on a focus-opened dot whose turn leaves canvas", () => {
    railCss.inject([
      ".turn-rail-preview",
      ".turn-rail-stop[data-preview-fits] .turn-rail-dot:hover .turn-rail-preview",
      ".turn-rail-stop[data-preview-fits][data-preview-open] .turn-rail-preview",
    ]);
    const buildStop = (fits: boolean): { stop: HTMLElement; preview: HTMLElement } => {
      const stop = document.createElement("span");
      stop.className = "turn-rail-stop";
      if (fits) stop.setAttribute("data-preview-fits", "");
      stop.setAttribute("data-preview-open", "");
      const dot = document.createElement("button");
      dot.className = "turn-rail-dot";
      const preview = document.createElement("span");
      preview.className = "turn-rail-preview";
      dot.appendChild(preview);
      stop.appendChild(dot);
      document.body.appendChild(stop);
      return { stop, preview };
    };
    // happy-dom serves a stale computed style when an element's attributes
    // change after a getComputedStyle call, so each state gets its own
    // stop instead of one stop mutated between reads.
    const fitting = buildStop(true);
    expect(getComputedStyle(fitting.preview).display).toBe("block");
    fitting.stop.remove();

    // No free canvas: even the focus-open must not cover the turn's bubble.
    const crowded = buildStop(false);
    expect(getComputedStyle(crowded.preview).display).toBe("none");
    crowded.stop.remove();

    // The hover path: happy-dom cannot compute :hover, so the rule text
    // is the sanctioned proof for it (cssProof.rulesFor).
    expect(
      railCss.rulesFor(
        ".turn-rail-stop[data-preview-fits] .turn-rail-dot:hover .turn-rail-preview",
      ),
    ).toContain("display: block");
  });

  it("keeps the dot a keyboard target with an accent focus ring and no motion", () => {
    railCss.inject([".turn-rail", ".turn-rail-dot", ".turn-rail-dot:focus-visible"]);
    const dot = document.createElement("button");
    dot.className = "turn-rail-dot";
    document.body.appendChild(dot);
    // Inert while the rail rests hidden — a touch tap cannot fire what it
    // cannot see — and answering under the same reveal as the rail itself.
    // Opacity hides without touching tab order, so the keyboard still lands.
    expect(getComputedStyle(dot).pointerEvents).toBe("none");
    expect(getComputedStyle(dot).backgroundColor).toBe("transparent");
    // WCAG 2.5.8's 24 px minimum target — only the button box is sized
    // here; the glyph inside stays the spec's 5/8/9.
    expect(getComputedStyle(dot).width).toBe("24px");
    expect(getComputedStyle(dot).height).toBe("24px");
    dot.remove();
    expect(railCss.rulesFor(".turn-rail-dot:focus-visible")).toContain("outline: 2px solid");
    expect(railCss.rulesFor(".turn-rail-dot:focus-visible")).toContain("#bd4a26");
    expect(railCss.rulesFor(".workspace-conversation:hover .turn-rail .turn-rail-dot")).toContain(
      "pointer-events: auto",
    );
    expect(
      railCss.rulesFor(".workspace-conversation:focus-within .turn-rail .turn-rail-dot"),
    ).toContain("pointer-events: auto");
    expect(railCss.rulesFor(".turn-rail.is-preview-open .turn-rail-dot")).toContain(
      "pointer-events: auto",
    );

    // The working pulse is the one animation this design defines; the rail
    // adds no transition or animation of its own.
    const railRules = railCss.rules.filter(
      (rule) => rule.selector.includes("turn-rail") || rule.selector.includes("has-turn-rail"),
    );
    expect(railRules.length).toBeGreaterThan(0);
    for (const rule of railRules) {
      expect(rule.body).not.toMatch(/transition|animation/);
    }
  });

  it("rests the rail invisible and answers hover, keyboard focus, or an open preview", () => {
    railCss.inject([".turn-rail"]);
    const rail = document.createElement("nav");
    rail.className = "turn-rail";
    document.body.appendChild(rail);
    // At rest the rail is out of the reading path but still in the tab
    // order: opacity hides, and happy-dom computes it.
    expect(getComputedStyle(rail).opacity).toBe("0");
    rail.remove();
    // Hover and keyboard focus cannot be computed, so the rule text is the
    // sanctioned proof for those two paths (cssProof.rulesFor).
    expect(railCss.rulesFor(".workspace-conversation:hover .turn-rail")).toContain("opacity: 1");
    expect(railCss.rulesFor(".workspace-conversation:focus-within .turn-rail")).toContain(
      "opacity: 1",
    );
    expect(railCss.rulesFor(".turn-rail.is-preview-open")).toContain("opacity: 1");
  });

  it("resolves the rail's theme tokens in the dark theme as well", () => {
    railCssDark.inject([
      ".turn-rail-thread",
      ".turn-rail-glyph",
      '.turn-rail-dot[aria-current="true"] .turn-rail-glyph',
    ]);
    expect(railCssDark.token("--line")).toBeDefined();
    expect(railCssDark.token("--accent-soft")).toBeDefined();
    expect(railCssDark.token("--accent-soft")).not.toBe(railCss.token("--accent-soft"));
    expect(railCssDark.token("--line")).not.toBe(railCss.token("--line"));
    expect(railCssDark.token("--tone-idle")).not.toBe(railCss.token("--tone-idle"));
    expect(railCssDark.token("--accent")).not.toBe(railCss.token("--accent"));

    const thread = document.createElement("span");
    thread.className = "turn-rail-thread";
    document.body.appendChild(thread);
    expect(getComputedStyle(thread).backgroundColor).toBe(railCssDark.token("--line"));
    thread.remove();

    // Same binding, dark theme: the assembled declaration carries the
    // resolved mix, which is what the contrast suite measures.
    expect(railCssDark.rulesFor(".turn-rail-glyph")).toContain(
      `color-mix(in srgb, ${railCssDark.token("--tone-idle")!} 40%, ${railCssDark.token("--ink")!})`,
    );

    const dot = document.createElement("button");
    dot.className = "turn-rail-dot";
    dot.setAttribute("aria-current", "true");
    const glyph = document.createElement("span");
    glyph.className = "turn-rail-glyph";
    dot.appendChild(glyph);
    document.body.appendChild(dot);
    const currentStyle = getComputedStyle(glyph);
    expect(currentStyle.backgroundColor).toBe(railCssDark.token("--accent"));
    expect(currentStyle.boxShadow).toContain("3px");
    // The ring is accent @ 16% in this theme: it carries the dark accent.
    expect(currentStyle.boxShadow).toContain(railCssDark.token("--accent")!);
    dot.remove();
  });
});
