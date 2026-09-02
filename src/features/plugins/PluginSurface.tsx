import { useEffect, useRef, useState } from "react";
import { createPluginBridge, HOST_SERVED_CAPABILITIES } from "./pluginBridge";
import { acquirePluginBackend, invokePlugin } from "./pluginBackend";
import { useAppStore } from "../../store/appStore";

const FRAME_START_TIMEOUT_MS = 15_000;

export interface PluginSurfaceProps {
  pluginId: string;
  entry: string | null;
  assetOrigin: string;
  capabilities: readonly string[];
}

export function PluginSurface({ pluginId, entry, assetOrigin, capabilities }: PluginSurfaceProps) {
  if (entry === null) {
    return (
      <PluginFailure
        pluginId={pluginId}
        reason={`${pluginId} is ready but did not declare a UI entry path`}
      />
    );
  }
  return (
    <PluginSurfaceContent
      pluginId={pluginId}
      entry={entry}
      assetOrigin={assetOrigin}
      capabilities={capabilities}
    />
  );
}

function PluginSurfaceContent({ pluginId, entry, assetOrigin, capabilities }: PluginSurfaceProps) {
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const frameTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const capabilitiesRef = useRef(capabilities);
  const origin = assetOrigin.replace(/\/+$/, "");
  const capabilitiesKey = JSON.stringify(capabilities);
  const refreshPlugins = useAppStore((state) => state.refreshPlugins);
  const [frameState, setFrameState] = useState<"starting" | "starting-long" | "ready" | "failed">(
    "starting",
  );
  const [reloadToken, setReloadToken] = useState(0);

  function markFrameReady() {
    if (frameTimeoutRef.current !== null) {
      clearTimeout(frameTimeoutRef.current);
      frameTimeoutRef.current = null;
    }
    setFrameState("ready");
  }

  function markFrameFailed() {
    if (frameTimeoutRef.current !== null) {
      clearTimeout(frameTimeoutRef.current);
      frameTimeoutRef.current = null;
    }
    setFrameState("failed");
  }

  useEffect(() => {
    capabilitiesRef.current = capabilities;
  }, [capabilities]);

  useEffect(() => {
    const iframe = iframeRef.current;
    if (iframe === null) return;
    const bridge = createPluginBridge({
      iframe,
      pluginId,
      pluginOrigin: origin,
      capabilities: capabilitiesRef.current,
      servedCapabilities: HOST_SERVED_CAPABILITIES,
      route: (method, payload) => invokePlugin(pluginId, method, payload),
    });
    const lease = acquirePluginBackend(pluginId);
    const timeout = setTimeout(() => {
      frameTimeoutRef.current = null;
      setFrameState("starting-long");
    }, FRAME_START_TIMEOUT_MS);
    frameTimeoutRef.current = timeout;
    iframe.addEventListener("load", markFrameReady);
    iframe.addEventListener("error", markFrameFailed);
    return () => {
      clearTimeout(timeout);
      if (frameTimeoutRef.current === timeout) frameTimeoutRef.current = null;
      iframe.removeEventListener("load", markFrameReady);
      iframe.removeEventListener("error", markFrameFailed);
      bridge.dispose();
      void lease.release();
    };
  }, [capabilitiesKey, origin, pluginId, reloadToken]);

  function rescan() {
    setFrameState("starting");
    setReloadToken((token) => token + 1);
    void refreshPlugins(true);
  }

  const label = pluginId === "polis" ? "Polis" : pluginId;

  return (
    <section className="surface-card plugin-surface" aria-label={`${pluginId} plugin surface`}>
      {/*
       * The plugin needs its own origin for postMessage replies and its own
       * storage. The familiar warning about combining allow-scripts with
       * allow-same-origin applies when the framed document is same-origin
       * with the embedder, because it could then remove its own sandbox
       * attribute. That is not this deployment: the embedder is
       * http://localhost:1420 in development or http://tauri.localhost in a
       * package, while the frame is http://plugin.localhost. The frame cannot
       * touch the parent document across those origins, and Tauri still
       * denies it IPC (GHSA-57fm-592m-34r7; patched in >= 2.0.0-beta.20;
       * this app uses 2.11.5). Here allow-same-origin means “keep your own
       * origin”, which is what the bridge and plugin storage require. The
       * sandbox still blocks top-level navigation, popups, form submission,
       * and downloads.
       */}
      <iframe
        ref={iframeRef}
        // The document and its bridge share a lifetime. The bridge effect
        // below re-runs on these same inputs and disposes the old bridge,
        // which drops any host-side sessions.watch subscriber; a document
        // that survived that rebuild would keep a subscription the host no
        // longer knows about and never learn to resubscribe. Remounting the
        // frame makes every bridge rebuild a document reload.
        key={`${pluginId}:${origin}:${capabilitiesKey}:${reloadToken}`}
        className="plugin-surface-frame"
        title={`${pluginId} plugin`}
        src={`${origin}/${pluginId}/${entry}`}
        sandbox="allow-scripts allow-same-origin"
      />
      {frameState === "failed" ? (
        <div className="polis-plugin-failure" role="alert">
          <span>{label} could not start — the plugin frame reported a loading error.</span>
          <button type="button" onClick={rescan}>
            Rescan
          </button>
        </div>
      ) : frameState === "starting-long" ? (
        <div className="polis-plugin-starting" role="status">
          {label} is still starting — the plugin has not reported that it is ready yet.
        </div>
      ) : null}
    </section>
  );
}

function PluginFailure({ pluginId, reason }: { pluginId: string; reason: string }) {
  return (
    <section className="surface-card plugin-surface" aria-label={`${pluginId} plugin surface`}>
      <p className="plugin-surface-status" role="alert">
        {reason}
      </p>
    </section>
  );
}
