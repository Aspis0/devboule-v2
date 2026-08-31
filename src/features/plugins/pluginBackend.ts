import { invokeTyped } from "../../lib/tauri";
import type { PluginBackendStatus } from "../../types/ipc";

/** Spawn (or ping) the plugin backend. First use of the Polis surface. */
export function ensurePluginBackend(pluginId: string): Promise<PluginBackendStatus> {
  return invokeTyped("plugin_backend_ensure", { plugin_id: pluginId });
}

/** Kill the backend when the surface closes. */
export function stopPluginBackend(pluginId: string): Promise<void> {
  return invokeTyped("plugin_backend_stop", { plugin_id: pluginId });
}

/** Forward a granted method over the plugin pipe. */
export function invokePlugin(
  pluginId: string,
  method: string,
  payload?: unknown,
): Promise<unknown> {
  return invokeTyped("plugin_invoke", { plugin_id: pluginId, method, payload });
}
