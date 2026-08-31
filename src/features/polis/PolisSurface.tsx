import { useEffect, useState } from "react";
import { SurfacePlaceholder } from "../../app/SurfacePlaceholder";
import { describePluginState, pluginState, pluginTone, POLIS_PLUGIN_ID } from "../../lib/plugins";
import {
  describePluginTransport,
  PLUGIN_ORIGINS,
  probePluginTransport,
  type PluginTransport,
} from "../../lib/pluginTransport";
import { useAppStore } from "../../store/appStore";
import type { SurfaceDefinition } from "../../types/surface";
import { chooseAndInstall } from "../plugins/install";
import { PluginSurface } from "../plugins/PluginSurface";

export function PolisSurface({ surface }: { surface: SurfaceDefinition }) {
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

  // The inventory is the app's, not this component's: the crescent decides
  // whether to draw a `+` from the same answer, and two fetches would be two
  // answers that can disagree. The Shell asks for it on the way in.
  const plugins = useAppStore((state) => state.plugins);
  const installing = useAppStore((state) => state.installing);
  const installError = useAppStore((state) => state.installError);
  const dismissInstallError = useAppStore((state) => state.dismissInstallError);
  const refreshPlugins = useAppStore((state) => state.refreshPlugins);
  const [checking, setChecking] = useState(false);

  const installed = plugins ? pluginState(plugins, POLIS_PLUGIN_ID) : null;
  const busy = checking || installing === POLIS_PLUGIN_ID;

  return (
    <>
      {installed?.kind === "ready" ? (
        <PluginSurface
          pluginId={POLIS_PLUGIN_ID}
          entry={installed.entry.uiEntry}
          assetOrigin={transport?.origin ?? PLUGIN_ORIGINS[0]}
          capabilities={installed.entry.capabilities}
        />
      ) : (
        <SurfacePlaceholder surface={surface} />
      )}
      {installed?.kind !== "ready" ? (
        <>
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
              load code it did not build. This checks that path end to end — policy, origin and
              content type — before anything depends on it.
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
            <p>
              {installed === null ? "Checking…" : describePluginState(installed, POLIS_PLUGIN_ID)}
            </p>
            <p className="polis-readiness-note">
              A plugin is a directory holding a manifest that lists every one of its files with a
              digest. Devboule reads nothing it was not told about, and a plugin whose files no
              longer match what the manifest describes is refused with a reason instead of half
              loaded.
            </p>
            {installError ? (
              <div className="polis-readiness-error" role="alert">
                <p className="polis-readiness-note polis-readiness-error">
                  The last install did not happen — {installError}
                </p>
                <button
                  className="polis-readiness-button"
                  type="button"
                  onClick={dismissInstallError}
                >
                  Dismiss
                </button>
              </div>
            ) : null}
            <div className="polis-readiness-actions">
              {installed?.kind === "absent" ? (
                <button
                  className="polis-readiness-button"
                  type="button"
                  disabled={busy}
                  onClick={() => void chooseAndInstall(POLIS_PLUGIN_ID, surface.label)}
                >
                  {installing === POLIS_PLUGIN_ID ? "Installing…" : "Install from a folder"}
                </button>
              ) : null}
              <button
                className="polis-readiness-button"
                type="button"
                disabled={busy}
                onClick={() => {
                  setChecking(true);
                  void refreshPlugins(true).finally(() => setChecking(false));
                }}
              >
                {checking ? "Looking…" : "Check again"}
              </button>
            </div>
          </section>
        </>
      ) : null}
    </>
  );
}
