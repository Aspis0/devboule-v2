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

## Depth: who draws over whom

Two facts that were each learned the expensive way. First, the kit art is
**front-anchored**: `makeProj(W, D)` puts the drawn footprint's front-bottom
corner at local (0,0), so a sprite extends up-screen from its position. The
layout therefore pins that position to the **front corner of the occupancy box**
(`cartToIso(gridX + fw, gridY + fh)`) — anchor it to the back corner and the
pixels cover tiles the road router believes are free, which is how roads once
ran under temple platforms.

Second, people never compete with buildings on `zIndex`. That was tried, with a
per-person epsilon, and no scalar epsilon can order a walker against a multi-tile
footprint (mid-face it loses by up to `fw + fh`). The v1's answer is structural
and is what this plugin does: painter order is `ground → roads → shadows →
crowd → buildings → monuments → agents → findings` (`mountWorldLayers`). The
ambient crowd and the porters live **below** buildings — with front-anchored
art, a walker in front of a building never overlaps it on screen, so "always
under" only ever clips walkers that are genuinely behind one. File-bound agents
stand on their buildings and live **above** them. `zIndex` orders only siblings:
building vs building (front ground corner, `buildingDepth`), walker vs walker
(ground `x + y`).

"Crowd under buildings" is only sound if no street touches a facade: a walker is
~23px tall against 24px tile rows, so on the facade's own ground line his whole
body sits inside the wall and vanishes. The router therefore masks the L of
front-face cells of every footprint (`roadGraph.ts`), and `BUILDING_STREET_GAP`
is 2 so each corridor keeps one routable row — with gap 1 the mask disconnected
82 doors (measured). This margin existed in the v1 only as an accident of its
art anchoring; here it is explicit and tested (`facadeRouting.test.ts`).

One more expensive fact: `http://plugin.localhost` serves the INSTALLED copy
under `%APPDATA%/com.devboule.desktop/plugins/<id>`, not this repo's `dist/`.
Rebuilding and reloading shows the old build; reinstall (or copy `dist/` over
the installed folder and restart the app) before believing a screenshot.

## Building

    pnpm install
    pnpm run build

`pnpm run build` extracts the fixture, typechecks, bundles, and writes the
manifest; `dist/` is what a user installs.
