import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { SurfacePlaceholder } from "../../app/SurfacePlaceholder";
import { describeGraphics, probeGraphics } from "../../lib/graphics";
import { describePluginState, pluginState, pluginTone, POLIS_PLUGIN_ID } from "../../lib/plugins";
import {
  describePluginTransport,
  probePluginTransport,
  type PluginTransport,
} from "../../lib/pluginTransport";
import { isCommandError, pluginsList, pluginsRescan } from "../../lib/tauri";
import type { PluginInventory } from "../../types/ipc";
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
  // The transport probe imports a module, so it cannot be synchronous. It runs
  // once and is never retried: the answer is a property of the build and the
  // policy, not of the moment.
  const [transport, setTransport] = useState<PluginTransport | null>(null);
  useEffect(() => {
    let cancelled = false;
    probePluginTransport().then((result) => {
      if (!cancelled) setTransport(result);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // Unlike the two probes above, what is installed can change while the app is
  // open, so this one is re-askable and guards its own writes.
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);
  const [inventory, setInventory] = useState<PluginInventory | null>(null);
  // True from the first render because the first look starts with the mount:
  // deriving it that way rather than setting it inside the effect keeps the
  // mount from costing a second render before anything has been asked.
  const [checking, setChecking] = useState(true);
  const apply = useCallback((found: PluginInventory) => {
    if (!alive.current) return;
    setInventory(found);
    setChecking(false);
  }, []);
  useEffect(() => {
    void look(false).then(apply);
  }, [apply]);

  const tone = !capability.webgl2
    ? "polis-readiness-blocked"
    : capability.softwareRendered === true
      ? "polis-readiness-degraded"
      : capability.softwareRendered === null
        ? "polis-readiness-unknown"
        : "polis-readiness-ready";
  const installed = inventory ? pluginState(inventory, POLIS_PLUGIN_ID) : null;

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
      <section
        className={`polis-readiness ${
          transport === null
            ? "polis-readiness-unknown"
            : transport.works
              ? "polis-readiness-ready"
              : "polis-readiness-blocked"
        }`}
        aria-label="Plugin loading"
      >
        <span className="polis-readiness-kicker">Installing Polis, when it exists</span>
        <p>{transport === null ? "Checking…" : describePluginTransport(transport)}</p>
        <p className="polis-readiness-note">
          Polis will be installed as files rather than compiled in, so the app has to be able to
          load code it did not build. This checks that path end to end — policy, origin and content
          type — before anything depends on it.
        </p>
      </section>
      <section
        className={`polis-readiness ${
          installed === null
            ? "polis-readiness-unknown"
            : `polis-readiness-${pluginTone(installed)}`
        }`}
        aria-label="Installed plugins"
      >
        <span className="polis-readiness-kicker">What is installed</span>
        <p>{installed === null ? "Checking…" : describePluginState(installed, POLIS_PLUGIN_ID)}</p>
        <p className="polis-readiness-note">
          A plugin is a directory holding a manifest that lists every one of its files with a
          digest. Devboule reads nothing it was not told about, and a plugin whose files no longer
          match what the manifest describes is refused with a reason instead of half loaded.
        </p>
        <button
          className="polis-readiness-button"
          type="button"
          disabled={checking}
          onClick={() => {
            setChecking(true);
            void look(true).then(apply);
          }}
        >
          {checking ? "Looking…" : "Check again"}
        </button>
      </section>
    </>
  );
}

/**
 * Ask what is installed, and always come back with an answer.
 *
 * The command already reports "I could not look" inside the inventory, so a
 * rejection here means the app itself did not answer. Folding that into the
 * same shape leaves the readout with one case to render instead of two.
 */
async function look(again: boolean): Promise<PluginInventory> {
  try {
    return again ? await pluginsRescan() : await pluginsList();
  } catch (cause) {
    return { root: "", plugins: [], problem: reasonFrom(cause) };
  }
}

function reasonFrom(cause: unknown): string {
  if (isCommandError(cause)) return cause.message;
  if (cause instanceof Error && cause.message) return cause.message;
  if (typeof cause === "string" && cause) return cause;
  return "the app did not answer";
}
