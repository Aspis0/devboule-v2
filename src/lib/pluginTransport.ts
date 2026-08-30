/**
 * Can this WebView load a plugin's code at all?
 *
 * M5 installs Polis as files the user drops in, not as code compiled into
 * Devboule. Those files are served by the app on a registered URI scheme
 * (`src-tauri/src/plugins/assets.rs`), which the platform puts on a **different
 * origin** from the app: `http://plugin.localhost` on Windows,
 * `plugin://localhost` elsewhere.
 *
 * Three things have to line up for a plugin module to load, and each fails in
 * its own way:
 *
 * - the Content-Security-Policy must list that origin in `script-src`,
 *   otherwise the import is blocked before a request is made;
 * - the response must carry `Access-Control-Allow-Origin`, because module
 *   scripts are always fetched in CORS mode — without it the failure reads as a
 *   CORS error rather than a missing file;
 * - the response must be typed as JavaScript, or the browser refuses the module
 *   without parsing it.
 *
 * Getting that wrong after tens of thousands of lines of Polis had moved would
 * be the expensive order to find out, so it is probed and reported instead.
 */

export interface PluginTransport {
  /** A module was imported from the plugin origin and evaluated. */
  works: boolean;
  /** The origin actually tried, which differs by platform. */
  origin: string;
  /** Why it did not work, in the browser's words. */
  reason: string | null;
}

/**
 * The origin the app's registered scheme resolves to.
 *
 * Windows and Android get `http://<scheme>.localhost`; the other platforms get
 * `<scheme>://localhost`. Rather than sniff the user agent, both are tried in
 * turn — a wrong guess would report a platform problem as a transport failure.
 */
export const PLUGIN_ORIGINS = ["http://plugin.localhost", "plugin://localhost"] as const;

const SELF_TEST_MODULE = "__selftest.js";

/**
 * Import the self test from each candidate origin until one evaluates.
 *
 * `importModule` is injectable because no test environment we run registers a
 * custom scheme, and a probe that cannot be tested is a probe nobody trusts.
 */
export async function probePluginTransport(
  importModule: (url: string) => Promise<unknown> = (url) => import(/* @vite-ignore */ url),
): Promise<PluginTransport> {
  let lastReason = "no origin was tried";
  for (const origin of PLUGIN_ORIGINS) {
    const url = `${origin}/${SELF_TEST_MODULE}`;
    try {
      const module = (await importModule(url)) as { pluginTransportWorks?: unknown };
      if (module?.pluginTransportWorks === true) {
        return { works: true, origin, reason: null };
      }
      lastReason = `${origin} served a module without the expected export`;
    } catch (error) {
      lastReason = `${origin}: ${describe(error)}`;
    }
  }
  return { works: false, origin: PLUGIN_ORIGINS[0], reason: lastReason };
}

function describe(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return "unknown error";
}

/** One line a person can read. */
export function describePluginTransport(transport: PluginTransport): string {
  return transport.works
    ? `Plugin code loads from ${transport.origin}`
    : `Plugin code cannot load — ${transport.reason ?? "no reason reported"}`;
}
