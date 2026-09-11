// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import {
  base64Length,
  collectAttachmentFiles,
  encodeSvgSourceBase64,
  formatAttachmentSize,
  importDesignAttachments,
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENT_COUNT,
  MAX_ATTACHMENT_TOTAL_BYTES,
  sanitizeSvgSource,
  sniffRasterMime,
  SVG_SANITIZER_RULES,
  svgSanitizerNotice,
  transferCarriesFiles,
} from "./designAttachments";
import type { DesignAttachment } from "./designHost";

function asciiBytes(text: string): number[] {
  return [...text].map((character) => character.charCodeAt(0));
}

/** One JPEG marker with a payload; the length field counts itself. */
function jpegSegment(marker: number, payload: readonly number[]): number[] {
  const length = payload.length + 2;
  return [0xff, marker, (length >> 8) & 0xff, length & 0xff, ...payload];
}

/** One PNG chunk; the four trailing bytes stand in for a CRC this code never rebuilds. */
function pngChunk(type: string, data: readonly number[]): number[] {
  const length = data.length;
  return [
    (length >>> 24) & 0xff,
    (length >>> 16) & 0xff,
    (length >>> 8) & 0xff,
    length & 0xff,
    ...asciiBytes(type),
    ...data,
    0xde,
    0xad,
    0xbe,
    0xef,
  ];
}

const JFIF_PAYLOAD = [
  ...asciiBytes("JFIF\u0000"),
  0x01,
  0x01,
  0x00,
  0x00,
  0x01,
  0x00,
  0x01,
  0x00,
  0x00,
];

// The fixtures are whole, walkable files because `importDesignAttachments` now
// walks every raster's segments before carrying it. A signature followed by
// arbitrary bytes is no longer an acceptable stand-in for a PNG or a JPEG.
const PNG_BYTES = Uint8Array.from([
  ...asciiBytes("\u0089PNG\r\n\u001a\n"),
  ...pngChunk("IHDR", [0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]),
  ...pngChunk("IDAT", [0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]),
  ...pngChunk("IEND", []),
]);
const JPEG_BYTES = Uint8Array.from([
  0xff,
  0xd8,
  ...jpegSegment(0xe0, JFIF_PAYLOAD),
  ...jpegSegment(0xda, [0x01, 0x01, 0x00, 0x00, 0x3f, 0x00]),
  0xff,
  0x00,
  0xff,
  0xd9,
]);

function pad(bytes: Uint8Array, size: number): Uint8Array<ArrayBuffer> {
  const padded = new Uint8Array(size);
  padded.set(bytes);
  return padded;
}

function rasterFile(name: string, bytes: Uint8Array<ArrayBuffer>, type: string): File {
  return new File([bytes], name, { type });
}

function svgFile(name: string, source: string, type = "image/svg+xml"): File {
  return new File([source], name, { type });
}

describe("attachment type is measured, never declared", () => {
  it("reads the PNG signature", () => {
    expect(sniffRasterMime(PNG_BYTES)).toBe("image/png");
  });

  it("reads the JPEG signature", () => {
    expect(sniffRasterMime(JPEG_BYTES)).toBe("image/jpeg");
  });

  it("has no answer for bytes that are neither", () => {
    expect(sniffRasterMime(new TextEncoder().encode("<svg/>"))).toBeNull();
  });

  it("attaches a PNG declared as a JPEG as a PNG and says so", async () => {
    const result = await importDesignAttachments(
      [rasterFile("shot.jpg", PNG_BYTES, "image/jpeg")],
      [],
    );

    expect(result.rejections).toEqual([]);
    expect(result.attachments[0]).toMatchObject({ kind: "raster", mimeType: "image/png" });
    expect(result.notices).toEqual([
      "shot.jpg declares image/jpeg, but its bytes are a PNG; it was attached as image/png.",
    ]);
  });

  it("attaches a JPEG named .png by its measured type", async () => {
    const result = await importDesignAttachments(
      [rasterFile("shot.png", JPEG_BYTES, "image/png")],
      [],
    );

    expect(result.attachments[0]).toMatchObject({ mimeType: "image/jpeg" });
    expect(result.notices[0]).toContain("it was attached as image/jpeg");
  });

  it("attaches an SVG named .png as an SVG document", async () => {
    const result = await importDesignAttachments(
      [svgFile("logo.png", '<svg xmlns="http://www.w3.org/2000/svg"></svg>', "image/png")],
      [],
    );

    expect(result.attachments[0]).toMatchObject({ kind: "svg", mimeType: "image/svg+xml" });
    expect(result.notices).toEqual([
      "logo.png declares image/png, but its bytes are an SVG document; it was attached as image/svg+xml.",
    ]);
  });

  it("stores raster bytes as base64 with no data prefix", async () => {
    const result = await importDesignAttachments([rasterFile("a.png", PNG_BYTES, "")], []);

    const attachment = result.attachments[0];
    expect(attachment.kind).toBe("raster");
    if (attachment.kind !== "raster") throw new Error("expected a raster");
    expect(attachment.base64).toBe(btoa(String.fromCharCode(...PNG_BYTES)));
    expect(attachment.base64).not.toContain("data:");
  });

  it("stores SVG source as text and not as base64", async () => {
    const result = await importDesignAttachments(
      [svgFile("logo.svg", '<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>')],
      [],
    );

    const attachment = result.attachments[0];
    expect(attachment.kind).toBe("svg");
    if (attachment.kind !== "svg") throw new Error("expected an svg");
    expect(attachment.source.startsWith("<svg")).toBe(true);
    expect(attachment.bytes).toBe(new TextEncoder().encode(attachment.source).length);
  });
});

function indexOfBytes(haystack: Uint8Array, needle: readonly number[]): number {
  outer: for (let start = 0; start + needle.length <= haystack.length; start += 1) {
    for (let offset = 0; offset < needle.length; offset += 1) {
      if (haystack[start + offset] !== needle[offset]) continue outer;
    }
    return start;
  }
  return -1;
}

describe("identity metadata is stripped on the way in", () => {
  const EXIF_PAYLOAD = [
    ...asciiBytes("Exif\u0000\u0000"),
    ...asciiBytes("GPSLatitude"),
    0x01,
    0x02,
    0x03,
    0x04,
  ];
  // SOI, JFIF, the EXIF APP1 a phone camera would write, then a scan whose
  // entropy data includes FF 00 stuffing and a bare RST marker.
  const EXIF_JPEG = Uint8Array.from([
    0xff,
    0xd8,
    ...jpegSegment(0xe0, JFIF_PAYLOAD),
    ...jpegSegment(0xe1, EXIF_PAYLOAD),
    ...jpegSegment(0xda, [0x01, 0x01, 0x00, 0x00, 0x3f, 0x00]),
    0x12,
    0x34,
    0xff,
    0x00,
    0xff,
    0xd0,
    0xff,
    0xd9,
  ]);

  it("carries shorter, EXIF-free bytes and says what left the file", async () => {
    const file = rasterFile("holiday.jpg", EXIF_JPEG, "image/jpeg");
    const result = await importDesignAttachments([file], []);

    expect(result.rejections).toEqual([]);
    const attachment = result.attachments[0];
    expect(attachment.kind).toBe("raster");
    if (attachment.kind !== "raster") throw new Error("expected a raster");

    const decoded = Uint8Array.from(atob(attachment.base64), (character) =>
      character.charCodeAt(0),
    );
    // Shorter than the file on disk, because the identity segment is gone, and the
    // recorded size is the carried size the ceilings are checked against.
    expect(decoded.length).toBeLessThan(file.size);
    expect(attachment.bytes).toBe(decoded.length);
    expect(indexOfBytes(decoded, asciiBytes("GPSLatitude"))).toBe(-1);
    // The pixels and their scan are untouched.
    expect(indexOfBytes(decoded, [0xff, 0xd0])).toBeGreaterThanOrEqual(0);
    expect(result.notices).toEqual([
      "holiday.jpg was stripped of metadata before attaching: the camera, device and GPS location data was removed.",
    ]);
  });

  it("refuses a raster it cannot walk rather than attaching it whole", async () => {
    const broken = rasterFile("broken.jpg", EXIF_JPEG.subarray(0, 8), "image/jpeg");
    const result = await importDesignAttachments([broken], []);

    expect(result.attachments).toEqual([]);
    expect(result.rejections[0].name).toBe("broken.jpg");
    expect(result.rejections[0].reason).toContain("broken.jpg was not added");
    expect(result.rejections[0].reason).toContain("free of hidden metadata");
  });
});

describe("nothing is dropped without a reason", () => {
  it("names the file and the reason when the bytes are not an image at all", async () => {
    const result = await importDesignAttachments(
      [new File(["just some text"], "notes.png", { type: "image/png" })],
      [],
    );

    expect(result.attachments).toEqual([]);
    expect(result.rejections).toHaveLength(1);
    expect(result.rejections[0].name).toBe("notes.png");
    expect(result.rejections[0].reason).toContain("notes.png");
    expect(result.rejections[0].reason).toContain("no PNG or JPEG signature");
    expect(result.rejections[0].reason).toContain("It declares image/png.");
  });

  it("refuses a PDF by name and points at what to do instead", async () => {
    const result = await importDesignAttachments(
      [
        new File([new Uint8Array([0x25, 0x50, 0x44, 0x46])], "brief.pdf", {
          type: "application/pdf",
        }),
      ],
      [],
    );

    expect(result.rejections[0].reason).toBe(
      "brief.pdf is a PDF, which this composer does not accept. Export the page as a PNG, or the artwork as an SVG.",
    );
  });

  it("refuses a file over the per-file ceiling and quotes both sizes", async () => {
    const result = await importDesignAttachments(
      [rasterFile("huge.png", pad(PNG_BYTES, MAX_ATTACHMENT_BYTES + 1), "image/png")],
      [],
    );

    expect(result.rejections[0].reason).toBe(
      "huge.png is 128.0 KB; one attached file may be at most 128.0 KB.",
    );
  });

  it("refuses an empty file", async () => {
    const result = await importDesignAttachments(
      [new File([], "empty.png", { type: "image/png" })],
      [],
    );

    expect(result.rejections[0].reason).toBe("empty.png is empty.");
  });

  it("refuses files past the count ceiling instead of attaching them quietly", async () => {
    const existing = Array.from({ length: MAX_ATTACHMENT_COUNT }, (_, index) => ({
      id: `id-${index}`,
      kind: "raster" as const,
      name: `file-${index}.png`,
      mimeType: "image/png" as const,
      bytes: PNG_BYTES.length,
      base64: "AA==",
    })) satisfies readonly DesignAttachment[];

    const result = await importDesignAttachments(
      [rasterFile("late.png", PNG_BYTES, "image/png")],
      existing,
    );

    expect(result.attachments).toEqual([]);
    expect(result.rejections[0].reason).toBe(
      `late.png was not added: at most ${MAX_ATTACHMENT_COUNT} files can be attached.`,
    );
  });

  it("refuses a file that would cross the total ceiling", async () => {
    const existing: readonly DesignAttachment[] = [
      {
        id: "id-0",
        kind: "raster",
        name: "first.png",
        mimeType: "image/png",
        bytes: MAX_ATTACHMENT_TOTAL_BYTES - 5,
        base64: "AA==",
      },
    ];

    const result = await importDesignAttachments(
      [rasterFile("second.png", PNG_BYTES, "image/png")],
      existing,
    );

    expect(result.rejections[0].reason).toContain("second.png was not added");
    expect(result.rejections[0].reason).toContain("add up to 256.0 KB");
  });

  it("reports a file already attached rather than adding it twice", async () => {
    const file = rasterFile("shot.png", PNG_BYTES, "image/png");
    const first = await importDesignAttachments([file], []);
    const second = await importDesignAttachments([file], first.attachments);

    expect(second.attachments).toEqual([]);
    expect(second.notices[0]).toBe("shot.png is already attached, so it was not added twice.");
  });

  it("recognizes the same SVG on a second import", async () => {
    // An SVG is stored with the length of its sanitized source and checked against
    // the same number, so the second drop of the same file is a duplicate — not, as
    // an earlier version had it, a silently re-added attachment.
    const file = svgFile("logo.svg", '<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>');
    const first = await importDesignAttachments([file], []);
    const second = await importDesignAttachments([file], first.attachments);

    expect(second.attachments).toEqual([]);
    expect(second.notices[0]).toBe("logo.svg is already attached, so it was not added twice.");
  });

  it("refuses XML whose root is not svg, naming the root it found", async () => {
    const result = await importDesignAttachments(
      [svgFile("page.svg", "<html><body>hi</body></html>", "image/svg+xml")],
      [],
    );

    expect(result.rejections[0].reason).toContain("its root element is <html>, not <svg>");
  });

  it("refuses malformed XML", async () => {
    const result = await importDesignAttachments(
      [svgFile("broken.svg", '<svg xmlns="http://www.w3.org/2000/svg"><rect></svg>')],
      [],
    );

    expect(result.rejections[0].reason).toContain("it is not well-formed XML");
  });
});

describe("the SVG sanitizer", () => {
  const HOSTILE = `<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" onload="steal()">
  <script>fetch("http://evil.example")</script>
  <foreignObject><iframe src="http://evil.example"></iframe></foreignObject>
  <a xlink:href="http://evil.example"><text>go</text></a>
  <image href="data:image/png;base64,AAAA"/>
  <use href="#safe"/>
  <rect fill="url(http://evil.example/p.png)"/>
  <rect style="fill: url('http://evil.example/p.png')"/>
  <set attributeName="href" to="http://evil.example"/>
  <animateTransform attributeName="transform" type="rotate" from="0" to="360" dur="1s"/>
  <style>@import url(http://evil.example/a.css); .k { fill: url(http://evil.example/p.png); }</style>
  <defs><linearGradient id="safe"/></defs>
</svg>`;

  it("removes every way the document could act, and reports which rules fired", () => {
    const result = sanitizeSvgSource(HOSTILE);
    expect(result.ok).toBe(true);
    if (!result.ok) return;

    expect([...result.removed].sort()).toEqual([
      "animationTarget",
      "eventHandler",
      "externalCssUrl",
      "externalReference",
      "foreignObject",
      "script",
    ]);
    expect(SVG_SANITIZER_RULES.script.reason).toContain("privileges");
  });

  it("gives every rule a user-facing label and a reason, from one definition", () => {
    for (const [rule, definition] of Object.entries(SVG_SANITIZER_RULES)) {
      expect(definition.label, rule).not.toBe("");
      expect(definition.reason, rule).not.toBe("");
      // A label names a thing that was taken out; a reason argues for the rule.
      expect(definition.label, rule).not.toBe(definition.reason);
    }
  });

  it("removes the SVG 1.2 handler and listener elements with their subtree", () => {
    // Nothing today executes these two, which is exactly the argument for not
    // leaning on today: the pass keeps what it has examined, and these carry
    // behaviour without having been examined.
    const result = sanitizeSvgSource(
      '<svg xmlns="http://www.w3.org/2000/svg"><handler id="h" type="text/ecmascript">fetch("http://evil.example")</handler><listener event="load" handler="#h"/><rect/></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).not.toContain("<handler");
    expect(result.source).not.toContain("<listener");
    // The text the handler carried goes with it.
    expect(result.source).not.toContain("evil.example");
    expect(result.source).toContain("<rect");
    expect(result.removed).toEqual(["svg12Handler"]);
  });

  it("takes the script and the foreignObject with their content", () => {
    const result = sanitizeSvgSource(HOSTILE);
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).not.toContain("<script");
    expect(result.source).not.toContain("fetch(");
    expect(result.source).not.toContain("foreignObject");
    expect(result.source).not.toContain("iframe");
  });

  it("removes every on* handler, not a list of known events", () => {
    const result = sanitizeSvgSource(
      '<svg xmlns="http://www.w3.org/2000/svg" onload="a()" ONCLICK="b()" oncustomevent="c()"><rect/></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).not.toMatch(/on\w+=/iu);
    expect(result.removed).toEqual(["eventHandler"]);
  });

  it("drops references that leave the document and keeps the self-contained ones", () => {
    const result = sanitizeSvgSource(HOSTILE);
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).not.toContain("http://evil.example");
    expect(result.source).toContain('href="#safe"');
    expect(result.source).toContain("data:image/png;base64,AAAA");
  });

  it("drops a javascript: reference", () => {
    const result = sanitizeSvgSource(
      '<svg xmlns="http://www.w3.org/2000/svg"><a href="javascript:alert(1)"><text>x</text></a></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).not.toContain("javascript:");
    expect(result.removed).toContain("externalReference");
  });

  it("keeps an inline payload only where it is rendered as a picture", () => {
    const result = sanitizeSvgSource(
      '<svg xmlns="http://www.w3.org/2000/svg"><image href="data:image/png;base64,AAAA"/><use href="data:image/svg+xml,%3Csvg/%3E"/><a href="data:text/html,%3Cscript%3E1%3C/script%3E"><text>x</text></a></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).toContain("data:image/png;base64,AAAA");
    // A <use> target and an <a> destination are documents, not pictures, and a
    // document can carry script.
    expect(result.source).not.toContain("data:image/svg+xml");
    expect(result.source).not.toContain("data:text/html");
    expect(result.removed).toEqual(["externalReference"]);
  });

  it("neutralizes an external url() in an attribute and in a style block", () => {
    const result = sanitizeSvgSource(HOSTILE);
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).toContain('fill="none"');
    expect(result.source).not.toContain("@import");
    // A data: url inside CSS is self-contained and must survive.
    const kept = sanitizeSvgSource(
      '<svg xmlns="http://www.w3.org/2000/svg"><rect fill="url(#grad)"/></svg>',
    );
    if (!kept.ok) throw new Error(kept.reason);
    expect(kept.source).toContain("url(#grad)");
  });

  it("drops a SMIL animation that would rewrite an href, and keeps the others", () => {
    const result = sanitizeSvgSource(HOSTILE);
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).not.toContain("<set");
    expect(result.source).toContain("animateTransform");
    expect(result.removed).toContain("animationTarget");
  });

  it("scrubs a url() an animation would write after load", () => {
    // The attribute itself is clean; the animation is what would install the fetch,
    // and it does so at playback, long after this pass has run.
    const result = sanitizeSvgSource(
      '<svg xmlns="http://www.w3.org/2000/svg"><rect width="8" height="8"><animate attributeName="style" values="fill:url(http://evil.example/a.png)" dur="1s" fill="freeze"/><set attributeName="fill" to="url(http://evil.example/b.png)"/></rect></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.source).not.toContain("evil.example");
    expect(result.source).toContain('values="fill:none"');
    expect(result.source).toContain('to="none"');
    // The animation itself is ordinary artwork and stays.
    expect(result.source).toContain("<animate");
    expect(result.removed).toContain("externalCssUrl");
  });

  it("strips the XML prologue so the string is an HTML fragment", () => {
    const result = sanitizeSvgSource(
      '<?xml version="1.0" encoding="UTF-8"?>\n<svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.source.startsWith("<svg")).toBe(true);
    expect(result.source).not.toContain("<?xml");
  });

  it("removes a doctype, including one carrying an entity", () => {
    const result = sanitizeSvgSource(
      '<!DOCTYPE svg PUBLIC "-//W3C//DTD SVG 1.1//EN" "http://www.w3.org/Graphics/SVG/1.1/DTD/svg11.dtd"><svg xmlns="http://www.w3.org/2000/svg"><rect/></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.source.toLowerCase()).not.toContain("<!doctype");
    expect(result.removed).toContain("doctype");
    // An internal subset can define entities, so it is refused outright rather
    // than sanitized: the parser itself is the risk at that point.
    const withEntity = sanitizeSvgSource(
      '<!DOCTYPE svg [<!ENTITY a "b">]><svg xmlns="http://www.w3.org/2000/svg"><text>&a;</text></svg>',
    );
    expect(withEntity.ok).toBe(false);
  });

  it("reports a clean document as clean", () => {
    const result = sanitizeSvgSource(
      '<svg xmlns="http://www.w3.org/2000/svg"><circle cx="1" cy="1" r="1"/></svg>',
    );
    if (!result.ok) throw new Error(result.reason);

    expect(result.removed).toEqual([]);
  });

  it("gives a reason instead of a crash for an empty string", () => {
    expect(sanitizeSvgSource("")).toMatchObject({ ok: false });
  });
});

describe("what the composer says about a file it sanitized", () => {
  it("names every rule that fired, once each, in one sentence", () => {
    const notice = svgSanitizerNotice("logo.svg", ["script", "eventHandler", "externalReference"]);

    expect(notice).toBe(
      "logo.svg was sanitized before attaching: a script, an event handler and a link to another site were removed.",
    );
    for (const label of ["a script", "an event handler", "a link to another site"]) {
      expect(notice.split(label)).toHaveLength(2);
    }
    // The rules' reasons belong in the source, not under a dropped logo.
    expect(notice).not.toContain("privileges");
    expect(notice).not.toContain("<script>");
    // The six lines this replaced ran to 430 characters.
    expect(notice.length).toBeLessThan(200);
  });

  it("reads as a list of one rather than a broken list", () => {
    const notice = svgSanitizerNotice("logo.svg", ["script"]);

    expect(notice).toBe("logo.svg was sanitized before attaching: a script was removed.");
    expect(notice).not.toContain(" and");
    expect(notice).not.toContain(", ");
  });

  it("has nothing to say when no rule fired", () => {
    expect(svgSanitizerNotice("logo.svg", [])).toBe("");
  });

  it("names a rule once even when it fires on several elements", async () => {
    const result = await importDesignAttachments(
      [
        svgFile(
          "logo.svg",
          '<svg xmlns="http://www.w3.org/2000/svg" onload="a()" onclick="b()"><script>c()</script><script>d()</script><rect/></svg>',
        ),
      ],
      [],
    );

    expect(result.attachments).toHaveLength(1);
    expect(result.notices).toEqual([
      "logo.svg was sanitized before attaching: a script and an event handler were removed.",
    ]);
  });
});

describe("collecting files from a drop or a paste", () => {
  const file = rasterFile("a.png", PNG_BYTES, "image/png");

  it("reads the files list", () => {
    const collected = collectAttachmentFiles({ files: [file] });

    expect(collected.files).toEqual([file]);
    expect(collected.unreadable).toBe(0);
  });

  it("falls back to the item list", () => {
    const collected = collectAttachmentFiles({
      files: [],
      items: [{ kind: "file", getAsFile: () => file }],
    });

    expect(collected.files).toEqual([file]);
  });

  it("counts an item that announces a file but yields none, such as a folder", () => {
    const collected = collectAttachmentFiles({
      files: [],
      items: [{ kind: "file", getAsFile: () => null }],
    });

    expect(collected.files).toEqual([]);
    expect(collected.unreadable).toBe(1);
  });

  it("ignores a string item so a text paste stays a text paste", () => {
    const collected = collectAttachmentFiles({
      files: [],
      items: [{ kind: "string", getAsFile: () => null }],
    });

    expect(collected.files).toEqual([]);
    expect(collected.unreadable).toBe(0);
  });

  it("has nothing to report for a null payload", () => {
    expect(collectAttachmentFiles(null)).toEqual({ files: [], unreadable: 0 });
  });
});

describe("deciding whether a payload is worth claiming", () => {
  it("claims a payload that lists a file", () => {
    expect(transferCarriesFiles({ files: [rasterFile("a.png", PNG_BYTES, "image/png")] })).toBe(
      true,
    );
  });

  it("claims a payload that only names the platform's own file marker", () => {
    expect(transferCarriesFiles({ files: [], types: ["Files"] })).toBe(true);
  });

  it("claims a payload whose only item announces a file but yields none", () => {
    expect(
      transferCarriesFiles({ files: [], items: [{ kind: "file", getAsFile: () => null }] }),
    ).toBe(true);
  });

  it("leaves a text payload alone so a text paste stays a text paste", () => {
    expect(transferCarriesFiles({ files: [], types: ["text/plain"] })).toBe(false);
    expect(
      transferCarriesFiles({ files: [], items: [{ kind: "string", getAsFile: () => null }] }),
    ).toBe(false);
  });

  it("has nothing to claim for a null payload", () => {
    expect(transferCarriesFiles(null)).toBe(false);
  });
});

describe("the numbers the limits are made of", () => {
  it("sizes a payload the way base64 does", () => {
    expect(base64Length(3)).toBe(4);
    expect(base64Length(4)).toBe(8);
  });

  it("encodes SVG source as base64 of its UTF-8 bytes", () => {
    expect(encodeSvgSourceBase64("<svg/>")).toBe("PHN2Zy8+");
    // The reason this is not plain `btoa`: the source is a string, and a
    // character outside ASCII has to survive as its UTF-8 bytes — what the
    // daemon writes to the file — not as a Latin-1 code point. The comparison
    // is against a Latin-1 encoding, because that is the bug this avoids.
    const source = "<svg><title>città</title></svg>";
    const bytes = Uint8Array.from(atob(encodeSvgSourceBase64(source)), (character) =>
      character.charCodeAt(0),
    );
    expect(bytes).toEqual(new TextEncoder().encode(source));
    expect(bytes).not.toEqual(
      Uint8Array.from(source, (character) => character.charCodeAt(0) & 0xff),
    );
    expect(new TextDecoder().decode(bytes)).toBe(source);
  });

  it("formats a size with one decimal and a unit", () => {
    expect(formatAttachmentSize(512)).toBe("512 B");
    expect(formatAttachmentSize(2048)).toBe("2.0 KB");
    expect(formatAttachmentSize(3 * 1024 * 1024)).toBe("3.0 MB");
  });

  it("keeps one inlined attachment's base64 inside three quarters of the artifact budget", () => {
    // Read the artifact budget out of its own file rather than restating it: if
    // MAX_ARTIFACT_BYTES moves, this relationship has to be re-decided, and a
    // test that hardcoded 256 KiB would keep passing while the reasoning rotted.
    const agentHost = readFileSync(
      join(dirname(fileURLToPath(import.meta.url)), "agentHost.ts"),
      "utf8",
    );
    const artifactBudget = Number(
      /export const MAX_ARTIFACT_BYTES = (\d+ \* \d+)/u
        .exec(agentHost)?.[1]
        ?.split(" * ")
        .map(Number)
        .reduce((left, right) => left * right, 1),
    );

    expect(artifactBudget).toBe(256 * 1024);
    expect(base64Length(MAX_ATTACHMENT_BYTES)).toBeLessThan(artifactBudget * 0.75);
  });

  it("holds the total ceiling at one artifact's worth", () => {
    expect(MAX_ATTACHMENT_TOTAL_BYTES).toBe(256 * 1024);
    expect(MAX_ATTACHMENT_BYTES * 2).toBe(MAX_ATTACHMENT_TOTAL_BYTES);
  });
});
