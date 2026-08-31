import { useEffect, useRef } from "react";
import { createPluginBridge } from "./pluginBridge";

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
  const origin = assetOrigin.replace(/\/+$/, "");

  useEffect(() => {
    const iframe = iframeRef.current;
    if (iframe === null) return;
    const bridge = createPluginBridge({
      iframe,
      pluginId,
      pluginOrigin: origin,
      capabilities,
    });
    return () => {
      bridge.dispose();
    };
  }, [capabilities, origin, pluginId]);

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
        className="plugin-surface-frame"
        title={`${pluginId} plugin`}
        src={`${origin}/${pluginId}/${entry}`}
        sandbox="allow-scripts allow-same-origin"
      />
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
