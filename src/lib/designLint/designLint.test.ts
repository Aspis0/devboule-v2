import { describe, expect, it } from "vitest";
import { lintSource } from "./designLint";

const TSX = "src/features/demo/Demo.tsx";
const GLOBAL_CSS = "src/styles/global.css";
const FEATURE_CSS = "src/features/demo/Demo.css";

function rules(findings: ReturnType<typeof lintSource>): string[] {
  return findings.map((f) => f.rule);
}

describe("lintSource / raw-hex", () => {
  it("flags a hex colour used in a component", () => {
    const findings = lintSource(".badge { color: #1c2a3a; }", TSX);
    expect(rules(findings)).toEqual(["raw-hex"]);
    expect(findings[0]?.severity).toBe("error");
  });

  it("names the matching palette token when the value matches exactly", () => {
    const findings = lintSource(".badge { color: #c8532b; }", TSX, {
      "--terracotta": "#c8532b",
    });
    expect(rules(findings)).toEqual(["raw-hex"]);
    expect(findings[0]?.message).toContain("--terracotta");
  });

  it("names the matching token for an rgb() literal too", () => {
    const findings = lintSource(".badge { color: rgb(200, 83, 43); }", TSX, {
      "--terracotta-rgb": "200, 83, 43",
    });
    expect(rules(findings)).toEqual(["raw-hex"]);
    expect(findings[0]?.message).toContain("--terracotta-rgb");
  });

  it("does not fire on var() usage", () => {
    const source = ".badge { color: var(--terracotta); }";
    expect(lintSource(source, TSX)).toEqual([]);
    expect(lintSource(source, FEATURE_CSS)).toEqual([]);
  });

  it("does not fire on hex inside a custom-property block of global.css", () => {
    const source = ":root {\n  --accent: #123456;\n}\nbody { color: #654321; }";
    const findings = lintSource(source, GLOBAL_CSS);
    expect(rules(findings)).toEqual(["raw-hex"]);
    // Only the body rule fires; the :root token block is exempt.
    expect(findings[0]?.line).toBe(4);
  });

  it("still fires on hex in a global.css rule that defines no custom properties", () => {
    const findings = lintSource("body { color: #654321; }", GLOBAL_CSS);
    expect(rules(findings)).toEqual(["raw-hex"]);
  });

  it("names the matching token when linting global.css itself", () => {
    const source = ":root {\n  --terracotta: #c8532b;\n}\n.footer { color: #c8532b; }";
    const findings = lintSource(source, GLOBAL_CSS);
    expect(rules(findings)).toEqual(["raw-hex"]);
    expect(findings[0]?.message).toContain("--terracotta");
  });

  it("reports the 1-indexed line and column of the literal", () => {
    const findings = lintSource("a\n.b { color: #c8532b; }", TSX);
    expect(findings[0]?.line).toBe(2);
    expect(findings[0]?.column).toBe(13);
  });

  it("does not flag an issue/PR number that looks like a hex triplet", () => {
    const source =
      'const [prLabel] = useState("Open #412 on GitHub");\n<span className="pr">#412</span>';
    expect(lintSource(source, TSX)).toEqual([]);
  });

  it("still flags a value in value position without a trailing semicolon", () => {
    const findings = lintSource(".badge {\n  color: #654321\n}", TSX);
    expect(rules(findings)).toEqual(["raw-hex"]);
  });

  it("names the token re-stated with alpha in an rgba() literal", () => {
    const findings = lintSource(".panel { border-color: rgba(200, 83, 43, 0.46); }", TSX, {
      "--terracotta-rgb": "200, 83, 43",
    });
    expect(rules(findings)).toEqual(["raw-hex"]);
    expect(findings[0]?.message).toContain("--terracotta-rgb");
    expect(findings[0]?.message).toContain("rgba(var(--terracotta-rgb), 0.46)");
  });
});

describe("lintSource / ai-default-indigo", () => {
  it("flags the default Tailwind indigo accent", () => {
    const findings = lintSource('const accent = "#6366f1";', TSX);
    expect(rules(findings)).toEqual(["ai-default-indigo"]);
    expect(findings[0]?.severity).toBe("error");
  });

  it("flags the violet variant", () => {
    expect(rules(lintSource('const accent = "#7c3aed";', TSX))).toEqual(["ai-default-indigo"]);
  });

  it("does not flag an off-list hex", () => {
    const findings = lintSource('const accent = "#8a3a1b";', TSX);
    expect(rules(findings)).not.toContain("ai-default-indigo");
  });

  it("does not fire on a colour built from design tokens", () => {
    expect(rules(lintSource(".cta { color: var(--accent); }", TSX))).toEqual([]);
  });
});

describe("lintSource / trust-gradient", () => {
  it("flags a blue-to-cyan two-stop gradient", () => {
    const source = 'style={{ background: "linear-gradient(90deg, #3b82f6, #06b6d4)" }}';
    const findings = lintSource(source, TSX);
    expect(rules(findings)).toEqual(["trust-gradient"]);
    expect(findings[0]?.severity).toBe("error");
  });

  it("flags an indigo-to-pink gradient written with keywords", () => {
    const source = 'style={{ background: "linear-gradient(135deg, indigo, pink)" }}';
    expect(rules(lintSource(source, TSX))).toEqual(["trust-gradient"]);
  });

  it("flags a purple-to-blue gradient in CSS", () => {
    const source = ".hero { background: linear-gradient(90deg, #8b5cf6, #3b82f6); }";
    expect(rules(lintSource(source, FEATURE_CSS))).toEqual(["trust-gradient"]);
  });

  it("does not fire on token-based gradients", () => {
    const source = ".hero { background: linear-gradient(90deg, var(--terracotta), var(--ochre)); }";
    expect(lintSource(source, TSX)).toEqual([]);
    expect(lintSource(source, FEATURE_CSS)).toEqual([]);
  });

  it("does not fire on a single-colour gradient", () => {
    const source = "linear-gradient(90deg, #3b82f6, #3b82f6)";
    expect(rules(lintSource(source, TSX))).not.toContain("trust-gradient");
  });

  it("suppresses the overlapping raw-hex and indigo findings on a flagged gradient", () => {
    const source = 'style={{ background: "linear-gradient(90deg, #6366f1, #ec4899)" }}';
    expect(rules(lintSource(source, TSX))).toEqual(["trust-gradient"]);
  });
});

describe("lintSource / emoji-icon", () => {
  it("flags an emoji used as button content", () => {
    const findings = lintSource('<button aria-label="Save">🚀 Save</button>', TSX);
    expect(rules(findings)).toEqual(["emoji-icon"]);
    expect(findings[0]?.severity).toBe("warning");
  });

  it("flags an emoji in a heading", () => {
    const findings = lintSource("<h3>✨ New feature</h3>", TSX);
    expect(rules(findings)).toEqual(["emoji-icon"]);
  });

  it("flags an emoji inside an element whose className names an icon", () => {
    const findings = lintSource('<span className="nav-icon">⚡</span>', TSX);
    expect(rules(findings)).toEqual(["emoji-icon"]);
  });

  it("does not flag emoji in prose", () => {
    const source = "<p>We shipped 🚀 to production today.</p>";
    expect(lintSource(source, TSX)).toEqual([]);
  });

  it("does not flag typographic symbols used in real UI", () => {
    const source = '<button aria-label="Add">✓ Add</button>';
    expect(lintSource(source, TSX)).toEqual([]);
  });
});

describe("lintSource / filler-copy", () => {
  it("flags lorem ipsum in visible copy", () => {
    const findings = lintSource("<p>Lorem ipsum dolor sit amet</p>", TSX);
    expect(rules(findings)).toEqual(["filler-copy"]);
    expect(findings[0]?.severity).toBe("error");
  });

  it("flags filler phrases in attribute copy", () => {
    const findings = lintSource('<h2 title="Sample content">Title</h2>', TSX);
    expect(rules(findings)).toEqual(["filler-copy"]);
  });

  it("does not flag a real input placeholder", () => {
    const source = '<input placeholder="Search the archive" />';
    expect(lintSource(source, TSX)).toEqual([]);
  });

  it("does not flag prose that merely contains a pattern word", () => {
    const source = "<p>Place your text here with care.</p>";
    expect(lintSource(source, TSX)).toEqual([]);
  });
});

describe("lintSource / invented-metric", () => {
  it("flags the canonical invented marketing numbers", () => {
    const cases = [
      "<span>10× faster</span>",
      "<span>99.9% uptime</span>",
      "<span>Zero-downtime deploys</span>",
      "<span>3x more productive</span>",
    ];
    for (const source of cases) {
      expect(rules(lintSource(source, TSX)), source).toEqual(["invented-metric"]);
      expect(lintSource(source, TSX)[0]?.severity).toBe("error");
    }
  });

  it("does not flag ordinary numbers in prose", () => {
    const source = "<p>The build finished 10 minutes faster.</p>";
    expect(lintSource(source, TSX)).toEqual([]);
  });

  it("does not fire on CSS files", () => {
    const source = ".hero::after { content: '10× faster'; }";
    expect(lintSource(source, FEATURE_CSS)).toEqual([]);
  });
});
