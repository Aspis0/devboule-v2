// @vitest-environment happy-dom

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  ATTACHMENT_INPUT_ACCEPT,
  base64Length,
  collectAttachmentFiles,
  countPdfPages,
  encodeSvgSourceBase64,
  formatAttachmentSize,
  importDesignAttachments,
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENT_COUNT,
  MAX_ATTACHMENT_TOTAL_BYTES,
  PDF_COUNT_PROBE_PAGE,
  pdfDocumentNotice,
  pdfPageBudget,
  pdfProgressNotice,
  pdfRefusalNotice,
  sanitizeSvgSource,
  sniffRasterMime,
  SVG_SANITIZER_RULES,
  svgSanitizerNotice,
  transferCarriesFiles,
} from "./designAttachments";
import type { DesignAttachment } from "./designHost";
import {
  PDF_MAX_FILE_BYTES,
  pdfTooLargeMessage,
  renderPdfPages,
  type PdfRenderOptions,
  type PdfRenderOutcome,
  type PdfRenderedPage,
} from "./pdfPageRenderer";

/**
 * The renderer is mocked for this file, and only `renderPdfPages` is replaced:
 * happy-dom has no 2d canvas, so a real render reports "could not be drawn" for
 * every page. Everything else the module exports — the sentences, the ceilings,
 * the header sniff — is the real thing, so the assertions below are made against
 * real wording and real numbers.
 */
vi.mock("./pdfPageRenderer", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./pdfPageRenderer")>();
  return { ...actual, renderPdfPages: vi.fn() };
});

/**
 * A stand-in for `pdfjs-dist`, so the real walk can be driven in one test below
 * without the library: a canvas cannot be drawn under happy-dom, so the
 * renderer's own tests stub the library at this same boundary. This one records
 * `getPage`, which is what the count probe must never reach.
 */
const pdfjsStub = vi.hoisted(() => {
  const state = { pageCount: 0, getPageCalls: 0 };
  return {
    state,
    module: {
      GlobalWorkerOptions: { workerSrc: "" },
      AnnotationMode: { DISABLE: 0 },
      getDocument: () => ({
        promise: Promise.resolve({
          numPages: state.pageCount,
          getPage: () => {
            state.getPageCalls += 1;
            return Promise.reject(new Error("the probe asked for a page"));
          },
        }),
        destroy: async () => undefined,
      }),
    },
  };
});

vi.mock("pdfjs-dist", () => pdfjsStub.module);

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

  it("refuses a file that declares a PDF and carries no PDF header", async () => {
    const result = await importDesignAttachments(
      [
        new File([new Uint8Array([0x25, 0x50, 0x44, 0x46])], "brief.pdf", {
          type: "application/pdf",
        }),
      ],
      [],
    );

    // Four of the five characters of the header. The name and the declared type
    // are not evidence of anything here — the bytes are — so this is not a
    // document, and nothing was rendered working that out. The sentence is the
    // renderer's own: a user who dropped a deck is told about the deck.
    expect(result.attachments).toEqual([]);
    expect(result.rejections[0].name).toBe("brief.pdf");
    expect(result.rejections[0].reason).toBe(
      "brief.pdf is not a readable PDF: it does not begin with the %PDF- header, and its bytes are not an image or an SVG document either.",
    );
    expect(renderPdfPages).not.toHaveBeenCalled();
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

const rendererMock = vi.mocked(renderPdfPages);

/** A whole PDF, because a document is read by its header and not by its name. */
const PDF_BYTES = Uint8Array.from([...asciiBytes("%PDF-1.4"), 0x0a]);

function pdfFile(name: string, bytes: Uint8Array<ArrayBuffer> = PDF_BYTES): File {
  return new File([bytes], name, { type: "application/pdf" });
}

function pageOf(pageNumber: number, bytes: number): PdfRenderedPage {
  return {
    pageNumber,
    width: 1,
    height: 1,
    scale: 1,
    mimeType: "image/jpeg",
    bytes: new Uint8Array(bytes),
  };
}

interface PageSizes {
  readonly pageNumber: number;
  readonly bytes: number;
}

function outcomeOf(input: {
  readonly name: string;
  readonly pageCount: number;
  readonly pages?: readonly PageSizes[];
  readonly omittedPages?: readonly number[];
  readonly downscaledPages?: readonly number[];
  readonly stoppedEarly?: PdfRenderOutcome["stoppedEarly"];
}): PdfRenderOutcome {
  return {
    name: input.name,
    pageCount: input.pageCount,
    pages: (input.pages ?? []).map((page) => pageOf(page.pageNumber, page.bytes)),
    omittedPages: input.omittedPages ?? [],
    downscaledPages: input.downscaledPages ?? [],
    stoppedEarly: input.stoppedEarly ?? null,
  };
}

/**
 * The renderer as this module uses it: a count call, then a render call. The
 * count is recognised by the page range it asks for, which is past the end of any
 * document, and answers with the page count alone. The render stamps out the
 * pages it was given and stops at an abort the way the real walk does, so a stop
 * decided in the sink is not followed by pages the renderer would never draw.
 */
function rendererServes(input: {
  readonly pageCount: number;
  readonly pages?: readonly PageSizes[];
  readonly omittedPages?: readonly number[];
  readonly downscaledPages?: readonly number[];
  readonly stoppedEarly?: PdfRenderOutcome["stoppedEarly"];
  readonly failure?: string;
}): void {
  rendererMock.mockImplementation(async (_bytes, name, sink, options) => {
    if (options?.pageRange?.from === PDF_COUNT_PROBE_PAGE) {
      return { ok: true, outcome: outcomeOf({ name, pageCount: input.pageCount }) };
    }
    if (input.failure !== undefined) return { ok: false, failure: { reason: input.failure } };
    const outcome = outcomeOf({
      name,
      pageCount: input.pageCount,
      pages: input.pages,
      omittedPages: input.omittedPages,
      downscaledPages: input.downscaledPages,
      stoppedEarly: input.stoppedEarly,
    });
    for (const page of outcome.pages) {
      if (options?.signal?.aborted === true) break;
      sink.onPage(page);
    }
    return { ok: true, outcome };
  });
}

/** The call that draws pages, as opposed to the one that only counts them. */
function renderLeg(): PdfRenderOptions | undefined {
  const call = rendererMock.mock.calls.find(
    ([, , , options]) => options?.pageRange?.from !== PDF_COUNT_PROBE_PAGE,
  );
  return call?.[3];
}

function countLegs(): number {
  return rendererMock.mock.calls.filter(
    ([, , , options]) => options?.pageRange?.from === PDF_COUNT_PROBE_PAGE,
  ).length;
}

/** Two full rasters: the composer's whole attachment budget, already spent. */
const FULL_COMPOSER: readonly DesignAttachment[] = ["a.png", "b.png"].map((name) => ({
  id: name,
  kind: "raster" as const,
  name,
  mimeType: "image/png" as const,
  bytes: MAX_ATTACHMENT_BYTES,
  base64: "AA==",
}));

describe("a PDF the composer carries as pictures of its pages", () => {
  beforeEach(() => {
    rendererMock.mockReset();
  });

  it("counts the document before it renders a page of it", async () => {
    rendererServes({
      pageCount: 2,
      pages: [
        { pageNumber: 1, bytes: 512 },
        { pageNumber: 2, bytes: 640 },
      ],
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], []);

    // The count comes first, and it asks for a page past the end on purpose:
    // that opens the document and draws nothing, which is what lets the composer
    // decide what fits before it spends a render finding out.
    expect(countLegs()).toBe(1);
    expect(rendererMock.mock.calls[0][3]?.pageRange).toEqual({
      from: PDF_COUNT_PROBE_PAGE,
      to: PDF_COUNT_PROBE_PAGE,
    });
    expect(renderLeg()?.pageRange).toEqual({ from: 1, to: 2 });
    expect(renderLeg()?.maxPages).toBe(2);
    expect(renderLeg()?.maxBytes).toBe(96 * 1024);
    expect(result.rejections).toEqual([]);
  });

  it("attaches the pages of one document and says it arrived whole", async () => {
    rendererServes({
      pageCount: 2,
      pages: [
        { pageNumber: 1, bytes: 512 },
        { pageNumber: 2, bytes: 640 },
      ],
    });
    const progress: string[] = [];

    const result = await importDesignAttachments([pdfFile("deck.pdf")], [], {
      onProgress: (line) => progress.push(line),
    });

    expect(result.notices).toEqual([
      "deck.pdf was attached in full: all 2 of its pages travel as pictures.",
    ]);
    expect(result.attachments.map((attachment) => attachment.name)).toEqual([
      "deck.pdf page 1 of 2",
      "deck.pdf page 2 of 2",
    ]);
    expect(result.attachments.map((attachment) => attachment.bytes)).toEqual([512, 640]);
    expect(result.attachments[0]).toMatchObject({ kind: "raster", mimeType: "image/jpeg" });
    const attachment = result.attachments[0];
    if (attachment.kind !== "raster") throw new Error("expected a raster page");
    // The picture is the page, byte for byte: nothing re-encodes it on the way in.
    const decoded = Uint8Array.from(atob(attachment.base64), (character) =>
      character.charCodeAt(0),
    );
    expect(decoded).toEqual(pageOf(1, 512).bytes);
    // Progress is a count and never a spinner: the work is countable.
    expect(progress).toEqual(["deck.pdf: page 1 of 2.", "deck.pdf: page 2 of 2."]);
  });

  it("refuses a document that does not fit, before rendering any of it", async () => {
    rendererServes({ pageCount: 12 });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], FULL_COMPOSER);

    expect(result.attachments).toEqual([]);
    // Nothing was rendered to reach this sentence: the only call was the count.
    expect(renderLeg()).toBeUndefined();
    expect(countLegs()).toBe(1);
    expect(result.rejections[0].name).toBe("deck.pdf");
    expect(result.rejections[0].reason).toBe(
      "deck.pdf has 12 pages, and none of them fits: one rendered page needs up to 96.0 KB and the composer has 0 B of its attachment budget free, so nothing was attached. Remove an attached file and attach the PDF again.",
    );
  });

  it("attaches what fits and names the pages the budget left out", async () => {
    rendererServes({
      pageCount: 5,
      pages: [
        { pageNumber: 1, bytes: 512 },
        { pageNumber: 2, bytes: 640 },
      ],
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], []);

    expect(result.attachments.map((attachment) => attachment.name)).toEqual([
      "deck.pdf page 1 of 5",
      "deck.pdf page 2 of 5",
    ]);
    expect(result.rejections).toEqual([]);
    expect(result.notices).toEqual([
      "deck.pdf was attached in part: pages 3-5 were left out because the composer's remaining attachment budget ran out after page 2, so 2 of its 5 pages travel as pictures.",
    ]);
    // Pages three onwards were never asked for, so they were never parsed for
    // rendering: the budget is decided before the walk, not discovered by it.
    expect(renderLeg()?.pageRange).toEqual({ from: 1, to: 2 });
  });

  it("names the pages the clock lost, apart from the ones the budget did", async () => {
    rendererServes({
      pageCount: 3,
      pages: [{ pageNumber: 1, bytes: 512 }],
      stoppedEarly: "timeout",
      omittedPages: [2],
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], []);

    expect(result.notices).toEqual([
      "deck.pdf was attached in part: page 3 was left out because the composer's remaining attachment budget ran out after page 1; page 2 was left out when the render ran out of time, so 1 of its 3 pages travels as a picture.",
    ]);
  });

  it("stops the walk when a page measures more than the composer can carry", async () => {
    rendererServes({
      pageCount: 2,
      pages: [
        { pageNumber: 1, bytes: MAX_ATTACHMENT_TOTAL_BYTES - 100 },
        { pageNumber: 2, bytes: 400 },
      ],
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], []);

    // The renderer's per-page ceiling is a target, not a guarantee: a page that
    // fits no rung of its ladder comes back anyway, so the ceiling that binds is
    // the composer's, measured against the bytes a page actually carries.
    expect(result.attachments.map((attachment) => attachment.bytes)).toEqual([
      MAX_ATTACHMENT_TOTAL_BYTES - 100,
    ]);
    expect(result.notices).toEqual([
      "deck.pdf was attached in part: page 2 was left out because the composer's remaining attachment budget ran out after page 1, so 1 of its 2 pages travels as a picture.",
    ]);
    expect(renderLeg()?.signal?.aborted).toBe(true);
  });

  it("passes through what the renderer says about a downscaled page", async () => {
    rendererServes({
      pageCount: 2,
      pages: [
        { pageNumber: 1, bytes: 512 },
        { pageNumber: 2, bytes: 640 },
      ],
      downscaledPages: [2],
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], []);

    expect(result.notices).toHaveLength(2);
    expect(result.notices[0]).toContain("attached in full");
    // The words are the renderer's: it is the one that knows what a downscale
    // cost, so the composer asks it rather than describing the page itself.
    expect(result.notices[1]).toContain("deck.pdf");
    expect(result.notices[1]).toContain("downscaled");
  });

  it("attaches nothing and says nothing when the user stops it", async () => {
    const controller = new AbortController();
    rendererMock.mockImplementation(async (_bytes, name, sink, options) => {
      if (options?.pageRange?.from === PDF_COUNT_PROBE_PAGE) {
        return { ok: true, outcome: outcomeOf({ name, pageCount: 3 }) };
      }
      sink.onPage(pageOf(1, 512));
      // Stopped while the first page was rendering: the renderer reports a
      // cancelled walk, and the page it already drew is not wanted either.
      controller.abort();
      return {
        ok: true,
        outcome: outcomeOf({
          name,
          pageCount: 3,
          pages: [{ pageNumber: 1, bytes: 512 }],
          omittedPages: [2],
          stoppedEarly: "cancelled",
        }),
      };
    });

    const result = await importDesignAttachments(
      [pdfFile("deck.pdf"), rasterFile("shot.png", PNG_BYTES, "image/png")],
      [],
      { signal: controller.signal },
    );

    expect(result.attachments).toEqual([]);
    expect(result.rejections).toEqual([]);
    expect(result.notices).toEqual([]);
  });

  it("reports a document already attached rather than adding it twice", async () => {
    rendererServes({ pageCount: 1, pages: [{ pageNumber: 1, bytes: 300 }] });
    const first = await importDesignAttachments([pdfFile("deck.pdf")], []);
    expect(first.attachments).toHaveLength(1);

    const second = await importDesignAttachments([pdfFile("deck.pdf")], first.attachments);

    expect(second.attachments).toEqual([]);
    expect(second.notices).toEqual(["deck.pdf is already attached, so it was not added twice."]);
  });

  it("refuses a document over the file ceiling without reading it", async () => {
    // The declaration is what makes a huge read worth attempting at all, so it is
    // checked against size before the bytes are pulled in: a 200 MB file named
    // .pdf is refused without ever reaching `arrayBuffer`.
    const huge = {
      name: "huge.pdf",
      type: "application/pdf",
      size: PDF_MAX_FILE_BYTES + 1,
      arrayBuffer: async (): Promise<ArrayBuffer> => {
        throw new Error("the file should not have been read");
      },
    } as unknown as File;

    const result = await importDesignAttachments([huge], []);

    expect(result.rejections[0].reason).toBe(
      pdfTooLargeMessage("huge.pdf", PDF_MAX_FILE_BYTES + 1),
    );
    expect(rendererMock).not.toHaveBeenCalled();
  });

  it("attaches a document far larger than the per-file ceiling", async () => {
    rendererServes({ pageCount: 1, pages: [{ pageNumber: 1, bytes: 100 }] });

    const result = await importDesignAttachments(
      [pdfFile("deck.pdf", pad(PDF_BYTES, MAX_ATTACHMENT_BYTES + 1))],
      [],
    );

    // The document is not what the composer carries: its pages are. A deck of
    // megabytes is one file that becomes a picture the budget can hold.
    expect(result.attachments).toHaveLength(1);
    expect(result.rejections).toEqual([]);
    expect(result.notices).toEqual([
      "deck.pdf was attached in full: its only page travels as a picture.",
    ]);
  });

  it("renders on the budget alone when the count cannot be taken", async () => {
    rendererMock.mockImplementation(async (_bytes, name, sink, options) => {
      if (options?.pageRange?.from === PDF_COUNT_PROBE_PAGE) {
        return {
          ok: false,
          failure: { reason: "deck.pdf is password-protected, so its pages could not be read." },
        };
      }
      const outcome = outcomeOf({
        name,
        pageCount: 4,
        pages: [
          { pageNumber: 1, bytes: 128 },
          { pageNumber: 2, bytes: 128 },
        ],
      });
      for (const page of outcome.pages) sink.onPage(page);
      return { ok: true, outcome };
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], []);

    // A count that could not be taken is not a reason to refuse a document: the
    // budget still bounds it, and the render's own outcome carries the count.
    expect(result.attachments.map((attachment) => attachment.name)).toEqual([
      "deck.pdf page 1 of 4",
      "deck.pdf page 2 of 4",
    ]);
    expect(result.notices).toEqual([
      "deck.pdf was attached in part: pages 3-4 were left out because the composer's remaining attachment budget ran out after page 2, so 2 of its 4 pages travel as pictures.",
    ]);
  });

  it("passes the renderer's own reason through when the render fails", async () => {
    rendererServes({
      pageCount: 3,
      failure: "deck.pdf needs image support this app does not ship.",
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], []);

    expect(result.attachments).toEqual([]);
    expect(result.rejections[0].reason).toBe(
      "deck.pdf needs image support this app does not ship.",
    );
  });

  it("passes the renderer's reason on when a document that cannot fit cannot be read", async () => {
    rendererMock.mockResolvedValue({
      ok: false,
      failure: { reason: "deck.pdf is password-protected, so its pages could not be read." },
    });

    const result = await importDesignAttachments([pdfFile("deck.pdf")], FULL_COMPOSER);

    expect(result.rejections[0].reason).toBe(
      "deck.pdf is password-protected, so its pages could not be read.",
    );
    expect(renderLeg()).toBeUndefined();
  });

  it("counts by opening the document and drawing nothing", async () => {
    rendererServes({ pageCount: 7 });

    await expect(countPdfPages(PDF_BYTES, "deck.pdf")).resolves.toEqual({
      ok: true,
      pageCount: 7,
    });
  });

  it("counts a real document by asking the renderer for a page past the last", async () => {
    // The probe is the one clever step in this file, so it is checked against the
    // real walk rather than against the mock the tests above use. `getPage`
    // rejects, so a probe that tried to draw anything would fail here; the
    // renderer's own page-range resolution is what makes this a count and not a
    // render, and this is where that dependency is pinned.
    pdfjsStub.state.pageCount = 7;
    pdfjsStub.state.getPageCalls = 0;
    const actual = await vi.importActual<typeof import("./pdfPageRenderer")>("./pdfPageRenderer");

    const result = await actual.renderPdfPages(
      PDF_BYTES,
      "deck.pdf",
      {
        onPage: () => {
          throw new Error("the probe drew a page");
        },
      },
      { pageRange: { from: PDF_COUNT_PROBE_PAGE, to: PDF_COUNT_PROBE_PAGE } },
    );

    expect(result.ok).toBe(true);
    if (!result.ok) throw new Error("expected a count");
    expect(result.outcome.pageCount).toBe(7);
    expect(result.outcome.pages).toEqual([]);
    expect(pdfjsStub.state.getPageCalls).toBe(0);
  });

  it("computes the page budget from the composer's own ceilings", () => {
    const raster = (bytes: number, name: string): DesignAttachment => ({
      id: name,
      kind: "raster",
      name,
      mimeType: "image/png",
      bytes,
      base64: "AA==",
    });
    const small = Array.from({ length: MAX_ATTACHMENT_COUNT - 1 }, (_, index) =>
      raster(1, `small-${index}.png`),
    );

    // 256 KiB of budget against a 96 KiB worst-case page is two pages, and four
    // slots is more than two: the bytes decide, and the slots only ever lower it.
    expect(pdfPageBudget([])).toBe(2);
    expect(pdfPageBudget([raster(MAX_ATTACHMENT_BYTES, "a.png")])).toBe(1);
    expect(pdfPageBudget(FULL_COMPOSER)).toBe(0);
    expect(pdfPageBudget(small)).toBe(1);
    expect(pdfPageBudget([...small, raster(1, "last.png")])).toBe(0);
  });

  it("has three sentences for three outcomes, and none of them is another", () => {
    const refused = pdfRefusalNotice({
      name: "deck.pdf",
      pageCount: 40,
      freeBytes: 0,
      firstLoss: "budget",
    });
    const whole = pdfDocumentNotice({
      name: "deck.pdf",
      pageCount: 2,
      attached: [1, 2],
      lostToBudget: [],
      lostToRender: [],
    });
    const part = pdfDocumentNotice({
      name: "deck.pdf",
      pageCount: 40,
      attached: [1, 2],
      lostToBudget: [3, 4, 5],
      lostToRender: [6],
    });

    expect(refused).toBe(
      "deck.pdf has 40 pages, and none of them fits: one rendered page needs up to 96.0 KB and the composer has 0 B of its attachment budget free, so nothing was attached. Remove an attached file and attach the PDF again.",
    );
    expect(whole).toBe("deck.pdf was attached in full: all 2 of its pages travel as pictures.");
    expect(part).toBe(
      "deck.pdf was attached in part: pages 3-5 were left out because the composer's remaining attachment budget ran out after page 2; page 6 was left out when the render ran out of time, so 2 of its 40 pages travel as pictures.",
    );
    expect(new Set([refused, whole, part]).size).toBe(3);
    expect(pdfProgressNotice("deck.pdf", 2, 3)).toBe("deck.pdf: page 2 of 3.");
  });

  it("offers a PDF in the picker and claims a drop that names one", () => {
    expect(ATTACHMENT_INPUT_ACCEPT.split(",")).toContain("application/pdf");
    expect(transferCarriesFiles({ files: [], types: ["application/pdf"] })).toBe(true);
  });
});
