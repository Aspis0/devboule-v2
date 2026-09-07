# Plugins

Out-of-process plugin host. A plugin is a folder with a manifest; there is no
registry or download. Installation (`install.ts`) asks for the folder the
plugin was unpacked into and installs it from there — the states around that
call (absent, installing, installed, error) are the same a download would
report.

`PluginSurface.tsx` mounts a plugin's UI in a cross-origin iframe with no
direct Tauri IPC access. Everything the frame may ask of the host goes through
`pluginBridge.ts`, the only channel into the host: a request is accepted only
when both the frame's origin and its identity match, is checked against a
host-side capability allowlist (`HOST_SERVED_CAPABILITIES`), and races a
timeout, because an awaited invoke from inside the frame never settles on its
own. Bridge methods route to typed host commands such as `sessions_list`,
`oracle_ask`, and `oracle_status`.

`pluginBackend.ts` owns the plugin's backend process: it spawns or pings it
through the typed `plugin_backend_ensure` command and hands out
reference-counted leases. A zero-delay release coalesces React StrictMode's
cleanup/remount pair, and a generation counter passed to the host stops an
older release from stopping a newer process.
