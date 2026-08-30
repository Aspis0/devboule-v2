/**
 * What this WebView can actually draw with.
 *
 * Polis renders an isometric city with PixiJS, which forces `preference: "webgl"`
 * and has no Canvas2D fallback. Devboule has never created a WebGL context in a
 * WebView2 window, so before M5 ports tens of thousands of lines the machine has
 * to answer two questions: can it make a WebGL2 context at all, and is that
 * context backed by the GPU or by a software rasteriser.
 *
 * The second question is the one that bites. Chromium historically fell back to
 * SwiftShader — a CPU implementation — rather than failing, which turns "does it
 * work" into "it works at a few frames per second", the kind of answer that only
 * shows up after the port. Newer Chromium is removing that automatic fallback in
 * favour of failing context creation outright, so both outcomes are live and the
 * probe reports which one happened instead of collapsing them into a boolean.
 */

export interface GraphicsCapability {
  /** A WebGL2 context could be created. */
  webgl2: boolean;
  /** Unmasked renderer string when the debug extension is available. */
  renderer: string | null;
  /** Unmasked vendor string when the debug extension is available. */
  vendor: string | null;
  /**
   * The renderer name matches a known CPU rasteriser. `null` means the name was
   * unavailable, which is not the same as "hardware" and must not be read as it.
   */
  softwareRendered: boolean | null;
  /** Present when `webgl2` is false: what went wrong, in the caller's words. */
  reason: string | null;
}

/** Renderer names that mean "no GPU is involved". */
const SOFTWARE_RENDERERS = [
  "swiftshader",
  "llvmpipe",
  "softpipe",
  "microsoft basic render",
  "generic renderer",
];

function looksLikeSoftware(renderer: string | null): boolean | null {
  if (!renderer) return null;
  const lower = renderer.toLowerCase();
  return SOFTWARE_RENDERERS.some((name) => lower.includes(name));
}

/**
 * Read the renderer strings. `WEBGL_debug_renderer_info` gives the real device;
 * without it `RENDERER` is a masked, generic string that cannot distinguish a
 * GPU from a rasteriser, so it is reported as-is and the software verdict stays
 * `null` rather than guessing.
 */
function readRendererStrings(context: WebGL2RenderingContext): {
  renderer: string | null;
  vendor: string | null;
  unmasked: boolean;
} {
  const debugInfo = context.getExtension("WEBGL_debug_renderer_info");
  if (debugInfo) {
    return {
      renderer: String(context.getParameter(debugInfo.UNMASKED_RENDERER_WEBGL) ?? "") || null,
      vendor: String(context.getParameter(debugInfo.UNMASKED_VENDOR_WEBGL) ?? "") || null,
      unmasked: true,
    };
  }
  return {
    renderer: String(context.getParameter(context.RENDERER) ?? "") || null,
    vendor: String(context.getParameter(context.VENDOR) ?? "") || null,
    unmasked: false,
  };
}

/**
 * Probe once. The caller is expected to memoise: creating and discarding
 * contexts is not free, and browsers cap how many can exist at a time.
 *
 * `createCanvas` is injectable so the probe can be tested without a real
 * WebGL implementation, which no test environment we run has.
 */
export function probeGraphics(
  createCanvas: () => HTMLCanvasElement = () => document.createElement("canvas"),
): GraphicsCapability {
  const absent = (reason: string): GraphicsCapability => ({
    webgl2: false,
    renderer: null,
    vendor: null,
    softwareRendered: null,
    reason,
  });

  let canvas: HTMLCanvasElement;
  try {
    canvas = createCanvas();
  } catch (error) {
    return absent(`could not create a canvas: ${describe(error)}`);
  }

  let context: WebGL2RenderingContext | null = null;
  try {
    context = canvas.getContext("webgl2") as WebGL2RenderingContext | null;
  } catch (error) {
    // Chromium throws rather than returning null when it refuses to fall back
    // to software rendering, so this branch is a real outcome, not paranoia.
    return absent(`WebGL2 context creation threw: ${describe(error)}`);
  }
  if (!context) {
    return absent("this WebView returned no WebGL2 context");
  }

  const { renderer, vendor, unmasked } = readRendererStrings(context);
  return {
    webgl2: true,
    renderer,
    vendor,
    softwareRendered: unmasked ? looksLikeSoftware(renderer) : null,
    reason: null,
  };
}

function describe(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return "unknown error";
}

/** One line a person can read, without hiding the uncertain cases. */
export function describeGraphics(capability: GraphicsCapability): string {
  if (!capability.webgl2) {
    return `WebGL2 unavailable — ${capability.reason ?? "no reason reported"}`;
  }
  const device = capability.renderer ?? "renderer not reported";
  if (capability.softwareRendered === true) {
    return `WebGL2 available but software-rendered (${device})`;
  }
  if (capability.softwareRendered === null) {
    return `WebGL2 available, hardware unconfirmed (${device})`;
  }
  return `WebGL2 available on ${device}`;
}
