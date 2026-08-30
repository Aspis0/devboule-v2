import { describe, expect, it } from "vitest";
import { describeGraphics, probeGraphics } from "./graphics";

const DEBUG_EXTENSION = {
  UNMASKED_RENDERER_WEBGL: 37446,
  UNMASKED_VENDOR_WEBGL: 37445,
};

const released: string[] = [];

function fakeCanvas(options: {
  context?: "throw" | "null" | "ok";
  extension?: boolean;
  renderer?: string;
  vendor?: string;
}): HTMLCanvasElement {
  const {
    context = "ok",
    extension = true,
    renderer = "NVIDIA GeForce RTX 4070",
    vendor = "NVIDIA",
  } = options;
  const parameters: Record<number, string> = {
    [DEBUG_EXTENSION.UNMASKED_RENDERER_WEBGL]: renderer,
    [DEBUG_EXTENSION.UNMASKED_VENDOR_WEBGL]: vendor,
    1: renderer,
    2: vendor,
  };
  return {
    getContext() {
      if (context === "throw") throw new Error("GPU process crashed");
      if (context === "null") return null;
      return {
        RENDERER: 1,
        VENDOR: 2,
        getExtension: (name: string) => {
          if (name === "WEBGL_lose_context") {
            return { loseContext: () => released.push(name) };
          }
          return extension ? DEBUG_EXTENSION : null;
        },
        getParameter: (name: number) => parameters[name] ?? "",
      };
    },
  } as unknown as HTMLCanvasElement;
}

describe("probeGraphics", () => {
  it("hands the context back instead of waiting for the collector", () => {
    released.length = 0;
    probeGraphics(() => fakeCanvas({}));
    // Browsers cap live WebGL contexts and evict the oldest; a probe that
    // leaks one can cost the renderer the context it is drawing with.
    expect(released).toContain("WEBGL_lose_context");
  });

  it("reports hardware when the renderer names a GPU", () => {
    const capability = probeGraphics(() => fakeCanvas({}));
    expect(capability.webgl2).toBe(true);
    expect(capability.softwareRendered).toBe(false);
    expect(capability.renderer).toBe("NVIDIA GeForce RTX 4070");
    expect(describeGraphics(capability)).toContain("available on NVIDIA");
  });

  it("names software rendering rather than calling it success", () => {
    const capability = probeGraphics(() =>
      fakeCanvas({ renderer: "Google SwiftShader (Subzero)" }),
    );
    expect(capability.webgl2).toBe(true);
    expect(capability.softwareRendered).toBe(true);
    expect(describeGraphics(capability)).toContain("software-rendered");
  });

  it("recognises the other CPU rasterisers by name", () => {
    for (const name of ["llvmpipe (LLVM 15)", "Microsoft Basic Render Driver", "softpipe"]) {
      expect(probeGraphics(() => fakeCanvas({ renderer: name })).softwareRendered).toBe(true);
    }
  });

  it("refuses to claim hardware when the renderer string is masked", () => {
    const capability = probeGraphics(() => fakeCanvas({ extension: false }));
    expect(capability.webgl2).toBe(true);
    // No debug extension means the name is generic; "not software" would be a
    // guess, and a guess here is exactly what would hide a slow Polis.
    expect(capability.softwareRendered).toBeNull();
    expect(describeGraphics(capability)).toContain("hardware unconfirmed");
  });

  it("treats a refused context as unavailable, with the reason", () => {
    const thrown = probeGraphics(() => fakeCanvas({ context: "throw" }));
    expect(thrown.webgl2).toBe(false);
    expect(thrown.reason).toContain("GPU process crashed");
    expect(describeGraphics(thrown)).toContain("unavailable");

    const empty = probeGraphics(() => fakeCanvas({ context: "null" }));
    expect(empty.webgl2).toBe(false);
    expect(empty.reason).toContain("no WebGL2 context");
  });

  it("survives a canvas it cannot even create", () => {
    const capability = probeGraphics(() => {
      throw new Error("document is not defined");
    });
    expect(capability.webgl2).toBe(false);
    expect(capability.reason).toContain("document is not defined");
  });
});
