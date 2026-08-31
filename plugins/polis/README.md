# Polis v2 — the codebase as a city, as a plugin

Devboule's first real plugin. It is not a port of the v1: the v1 had its own agent
system, and the v2 is an orchestrator over existing CLI agents, so the data model
is different even where the pictures look alike. See `ARCHITETTURA.md` §5.6.

## What it draws today

Buildings are files — footprint and height from line count, tint from the top-level
folder. Roads are imports, thickness from weight, direction importer → imported.
The city is this repository, extracted by `scripts/extract-city-fixture.mjs`.

**That extractor is a stand-in and is named as one.** The real graph is the CKG the
host already builds (`ARCHITETTURA.md` §5.5), and it will arrive over the bridge as
a capability. The regex extractor exists so the renderer could be built and looked
at before that seam is finished.

## What it runs inside

A cross-origin iframe on `http://plugin.localhost`, with no Tauri IPC and a
Content-Security-Policy the host sends with the document. Two consequences that
cost real time to find, both measured rather than reasoned:

- **PixiJS needs `pixi.js/unsafe-eval`.** Without it the renderer refuses to start
  under the policy. This does not show up on a static server, which has no policy.
- **Anything you await from the host must race a timeout.** An `invoke` from inside
  the frame never settles — it does not succeed and it does not reject. One hung
  await once blocked an unrelated readout in this very plugin.

## The overlay is not decoration

It reports three facts measured from inside the frame: whether WebGL2 started and
on what renderer, what happens when the plugin tries to reach Tauri directly, and
what the host answered on the bridge. Both of the problems above were found by
reading it, not by reading code. Keep it honest and keep it visible.

## Building

    pnpm install
    pnpm run build
    node ../../scripts/make-plugin-manifest.mjs dist --entry-ui index.html --write

`dist/` is what a user installs.
