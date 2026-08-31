# Polis Art Credits

The isometric city view ("Polis") renders with open-licensed sprite art from the
sources below, curated and in some cases rescaled/recolored to fit the app's
palette. This file ships **inside the Polis plugin**, which is what distributes the art:
Devboule itself carries none of it, and uninstalling Polis removes both the
sprites and this notice together.

Provenance for every key lives in `plugins/polis/public/atlas/*.json` beside the
pages themselves, and the jobs that produced them — source project, exact input
path, and per-frame anchors — are recorded in the v1 pipeline specs at
`old-devboule/tools/polis-art/specs/`. An asset without a recorded source must
not be committed.

License notes:
- **CC0** assets require no attribution (credited anyway, with thanks).
- **CC-BY** assets are credited per their license.
- **CC-BY-SA** art: any modified sprite files we distribute remain licensed
  CC-BY-SA (the share-alike applies to those art files only, not to this
  application). Modified files are flagged in the ledger.
- **CC-BY-ND** assets are bundled unmodified.

## Sources

- **Screaming Brain Studios** — seamless terrain/material textures (grass,
  dirt, stone, plaster, ashlar/brick, marble, wood planks, terracotta roof
  tiles, thatch, sea water + underwater caustics; `tex:*`) from the "Tiny Texture Pack" series.
  https://opengameart.org/content/tiny-texture-pack — License: **CC0**
  ("released under the CC0/Public Domain License", pack License.txt).
  Used rescaled, some desaturated/brightened to fit the palette. Thank you!

- **Unknown Horizons team** — tree sprites (`prop:tree`, `prop:cypress`)
  and the countryside resource art: mountain mine, stone-deposit and stone-pit
  quarries, ambient rocks (`res:mine`, `res:quarry:*`, `prop:rock:*`).
  https://github.com/unknown-horizons/unknown-horizons (content/gfx) —
  License: **CC-BY-SA 3.0** (art, per the project's doc/LICENSE; multiple
  artists — see their doc/AUTHORS.md). Rescaled/re-packed for this app; the
  modified sprite files remain CC-BY-SA per the share-alike note above.

- **FoshyTakashi** — 9-frame fire animation (`fx:fire:*`, the burning-building
  flip-book frames).
  https://opengameart.org/content/9-frame-fire-animation-16x-32x-64x —
  License: **CC-BY 3.0**. Frames cut from the 64px strip, rescaled and tinted
  per fire severity. Thank you!
