import { useMemo } from "react";
import { SurfacePlaceholder } from "../../app/SurfacePlaceholder";
import { describeGraphics, probeGraphics } from "../../lib/graphics";
import type { SurfaceDefinition } from "../../types/surface";

/**
 * The Polis surface before Polis exists.
 *
 * It shows the placeholder, plus the one fact about this machine that decides
 * whether the M5 port is worth starting here: Polis draws with PixiJS, which
 * forces WebGL and has no Canvas2D fallback, and Devboule has never created a
 * WebGL context in a WebView2 window. Answering that after porting tens of
 * thousands of lines would be the expensive order to find out.
 *
 * It is deliberately not a gate. Nothing is disabled by what it reports; it is
 * a readout, in the same spirit as Oracle's doctor.
 */
export function PolisSurface({ surface }: { surface: SurfaceDefinition }) {
  // Probed once per mount: creating and discarding WebGL contexts is not free
  // and browsers cap how many may exist at a time.
  const capability = useMemo(() => probeGraphics(), []);
  const tone = !capability.webgl2
    ? "polis-readiness-blocked"
    : capability.softwareRendered === true
      ? "polis-readiness-degraded"
      : capability.softwareRendered === null
        ? "polis-readiness-unknown"
        : "polis-readiness-ready";

  return (
    <>
      <SurfacePlaceholder surface={surface} />
      <section className={`polis-readiness ${tone}`} aria-label="Polis rendering requirements">
        <span className="polis-readiness-kicker">This machine, for M5</span>
        <p>{describeGraphics(capability)}</p>
        <p className="polis-readiness-note">
          {capability.webgl2
            ? capability.softwareRendered === true
              ? "The isometric view would run on the CPU here. It will draw, slowly."
              : "PixiJS runs under this window's strict content policy: it imports the patch that removes its use of new Function, so the policy stays closed."
            : "The isometric view needs WebGL2 and has no 2D fallback. It would not draw on this machine."}
        </p>
      </section>
    </>
  );
}
