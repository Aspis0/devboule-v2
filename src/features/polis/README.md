# Polis surface

The Polis plugin is built and mounted. This surface is its host shell: it
probes the plugin transport once, reads the plugin inventory from the app
store, and renders the plugin frame through the shared out-of-process
`PluginSurface` host when Polis is installed — or a placeholder plus
installation and transport diagnostics when it is not (`PolisSurface.tsx`).
The plugin itself lives in `plugins/polis/`.
