# Third-party notices

Devboule v2 project code is distributed under Apache-2.0; see LICENSE and NOTICE.
This file is the attribution inventory for the repository state audited on 2026-08-27.
It distinguishes linked dependencies, source copied into the repository, and non-code assets.

## How this inventory was built

The Rust inventory is the complete set of 470 registry packages resolved by the workspace Cargo.toml files and Cargo.lock with all features enabled. The JavaScript inventory is the complete set of 156 package records in pnpm-lock.yaml. Direct versus transitive/build/test/optional status is derived from the workspace manifests and package.json.

The licence column preserves the package-declared licence expression. OR and AND are not rewritten into a simpler label. The Rust metadata comes from cargo metadata; npm metadata comes from the package metadata of the exact versions pinned in pnpm-lock.yaml, read from the local install where materialized and from the exact registry package metadata for the same version otherwise. The local pnpm store still holds directories for packages the lockfile no longer references after the frontend toolchain change, so the installed tree was not used to decide which records belong in the inventory. The lockfiles remain the version and source authority.

The lockfile inventories include conditional platform packages. A package listed in the lockfile is not necessarily linked into every target artifact; on Windows, only the Windows target branches are selected. Build/test dependencies are not shipped as application runtime code.

## Copied or vendored source code

One third-party file is vendored as **data, not code**: `crates/devboule-augur/vendor/gitleaks/gitleaks.toml`, the gitleaks secret-detection rule set taken at tag v8.21.2, commit 43fae355e6fe4d99d2a7b240a224b85e2903aeb4, under the MIT licence (Copyright (c) 2019 Zachary Rice). Its LICENSE ships beside it and `VERSION` records the upstream repository, tag, commit and the date taken. No Go code is used; the TOML is parsed and the regexes compiled by our own loader.

Beyond that there is no third-party library source code checked into this repository: no Cargo patch/fork, no git submodule, and no third-party path dependency; the path dependencies are the local Devboule workspace crates.

Third-party **sprite art** is a separate matter and is inventoried under Non-code assets below. It is carried by the Polis plugin rather than by the application, so an installation without Polis distributes none of it.

The esaxx-rs one-line CRT patch mentioned in the separate architecture document is not present here: this repository has no esaxx-rs reference, no oracle-core member, no fastembed dependency, and no corresponding build.rs patch. The oracle-core names in mock UI data are not source code.

The application does link/compile code from the registry dependencies listed below. In particular, rusqlite 0.40.2 is enabled with bundled: libsqlite3-sys 0.38.2 compiles the SQLite amalgamation it ships, SQLite 3.53.2, into the daemon artifact. That source is not checked into this repository, but it is part of the linked build input; rusqlite and libsqlite3-sys declare MIT, while the bundled SQLite amalgamation itself is distributed by SQLite under its public-domain dedication.

## Non-code assets

| File | Provenance / use | Copyright | Licence | SHA-256 |
| --- | --- | --- | --- | --- |
| src/styles/fonts/Fraunces-Latin.woff2 | Fontsource Variable 5.3.0 asset; upstream [Fraunces](https://github.com/undercasetype/Fraunces); package: [@fontsource-variable/fraunces](https://www.npmjs.com/package/@fontsource-variable/fraunces/v/5.3.0) | Copyright 2020 The Fraunces Project Authors | SIL OFL 1.1 | 7F9D191D999336D3B9790AFA72E1358E50A13B06D4F289341E92A311967A80F9 |
| src/styles/fonts/Inter-Latin.woff2 | Fontsource Variable 5.3.0 asset; upstream [Inter](https://github.com/rsms/inter); package: [@fontsource-variable/inter](https://www.npmjs.com/package/@fontsource-variable/inter/v/5.3.0) | Copyright 2016 The Inter Project Authors | SIL OFL 1.1 | 3100E775E8616CD2611BEECFA23A4263D7037586789B43F035236A2E6FBD4C62 |
| src/styles/fonts/JetBrainsMono-Latin.woff2 | Fontsource Variable 5.3.0 asset; upstream [JetBrains Mono](https://github.com/JetBrains/JetBrainsMono); package: [@fontsource-variable/jetbrains-mono](https://www.npmjs.com/package/@fontsource-variable/jetbrains-mono/v/5.3.0) | Copyright 2020 The JetBrains Mono Project Authors | SIL OFL 1.1 | 18BE452724BFDC236C074CA94A249A7F41A86752C7D04AB258CE9ED5651F6A7E |
| src-tauri/icons/icon.ico | Devboule application icon. Original project artwork: a Greek key meander ring around a terminal prompt glyph, with the "devboule" wordmark. Not derived from the Tauri template icon set. | Copyright 2026 Aspis0 | Same licence as this project (Apache-2.0) | BB774A6D1CFB1A61D921CF1952E32EB4C6995637A784BF77DFFCA280897F805D |
| plugins/polis/public/atlas/fx-0.json | Nine-frame fire flip-book from [9-frame fire animation](https://opengameart.org/content/9-frame-fire-animation-16x-32x-64x); frames cut from the 64px strip, rescaled and tinted per severity; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | FoshyTakashi | CC-BY 3.0 | 19CBCDDE9C8B974B8AD1F9949DF1DBAD9EDDAFBCB2A3FCDF43541EA212874B28 |
| plugins/polis/public/atlas/fx-0.png | Nine-frame fire flip-book from [9-frame fire animation](https://opengameart.org/content/9-frame-fire-animation-16x-32x-64x); frames cut from the 64px strip, rescaled and tinted per severity; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | FoshyTakashi | CC-BY 3.0 | 77FC6BFC837942E2167AEA940B7C79190C7C4C56E24A714DCF12FA4A36A75BF3 |
| plugins/polis/public/atlas/prop-0.json | Tree, cypress and rock sprites from [Unknown Horizons](https://github.com/unknown-horizons/unknown-horizons) `content/gfx`; rescaled and re-packed. **Modified files remain CC-BY-SA**; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Unknown Horizons team (multiple artists; see their doc/AUTHORS.md) | CC-BY-SA 3.0 | A41ACD3E73D220BE9378A2FF34A00C9DDE6981E46496A3FFC32CE3743E7A98E4 |
| plugins/polis/public/atlas/prop-0.png | Tree, cypress and rock sprites from [Unknown Horizons](https://github.com/unknown-horizons/unknown-horizons) `content/gfx`; rescaled and re-packed. **Modified files remain CC-BY-SA**; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Unknown Horizons team (multiple artists; see their doc/AUTHORS.md) | CC-BY-SA 3.0 | 1F69449E2256BA5A1AA9CB270D1B05535757050216CBC2CABCD73C87951C6D16 |
| plugins/polis/public/atlas/res-0.json | Mine and quarry sprites from [Unknown Horizons](https://github.com/unknown-horizons/unknown-horizons) `content/gfx`; rescaled and re-packed. **Modified files remain CC-BY-SA**; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Unknown Horizons team (multiple artists; see their doc/AUTHORS.md) | CC-BY-SA 3.0 | D7BC3FD5FA3CF616FF11BFBB93BD6BD4D3BCE190AFB5D39A645614F983D33E4F |
| plugins/polis/public/atlas/res-0.png | Mine and quarry sprites from [Unknown Horizons](https://github.com/unknown-horizons/unknown-horizons) `content/gfx`; rescaled and re-packed. **Modified files remain CC-BY-SA**; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Unknown Horizons team (multiple artists; see their doc/AUTHORS.md) | CC-BY-SA 3.0 | AF855BD86B6B7C74037CF8290340AAEB137A028672354D56B0E4929E89AA0926 |
| plugins/polis/public/atlas/tex__ashlar.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | DB1BCE7729C3B43C49D0A6849AEE8DCAE52B6E814136CA46A833AA10F7C4EFCD |
| plugins/polis/public/atlas/tex__cobble.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 908F827C2C4953A8B57A535245283D6DA9882EF4418E51B33516FB285BAABA4C |
| plugins/polis/public/atlas/tex__dirt.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 8C6F90B3B2A5A6FC611B8BB4A08B74998E97DF34AA814688FCB36F1FD3D38AB9 |
| plugins/polis/public/atlas/tex__dirtolive.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | C8AC39C84340F63B0132D4B92DECBB58C7AA92270303F5A9CC962D2F852A4EE2 |
| plugins/polis/public/atlas/tex__grass.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 780780005D42C964292EA82938AC85F9C3529916E4ABA8B80AD9254AA87FDC9A |
| plugins/polis/public/atlas/tex__grassdark.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | AF0283D89C2B3E402D03618E8174323BF5827F84F6516543B0A171B95ADF7E52 |
| plugins/polis/public/atlas/tex__grassdry.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | F510C9C5FA9F0A0991B6D0B766C52DBF0AB919349F2EC948328D5585B6F84FAB |
| plugins/polis/public/atlas/tex__marble.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 58E36FA4BA349D317397C9D27D9429C616921E10A4FAB432E675DF8BF6D2BF64 |
| plugins/polis/public/atlas/tex__plaster.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | CD9E7A59D051B9FF702F133B5FC366DF0D1ED0FC4AC074F3746A766369DABD63 |
| plugins/polis/public/atlas/tex__plasterwarm.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 9067C3E8289A3F7A3D8E877D4E32CE5D2084B152696204840B4784A1A572B6B7 |
| plugins/polis/public/atlas/tex__rooftile.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 1F3342A75ABBC052F53F7B0DE31E60A1DFC4D7E75C10A43D0CF2E32414AD233C |
| plugins/polis/public/atlas/tex__stonegrey.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 144ED000D1DA3FE4E51D8939C4EA7DF8A8AC768D7F59A040D5E9FB88D797DE26 |
| plugins/polis/public/atlas/tex__thatch.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 56B6D7BD835AE8A6D3FC0661CCCFE37B76227ED5D72E8E4C3C2346AD8E745887 |
| plugins/polis/public/atlas/tex__water.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 9FC4F57A67320C1BFF9B0911BB57DEE10BB05FA91CB78574317074D5F092E0C2 |
| plugins/polis/public/atlas/tex__waterdeep.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 3F1449348A15E4950C2C659EC2FD2EDD2E324830FDABCB4494BA8AF2117BF188 |
| plugins/polis/public/atlas/tex__wood.png | Seamless material texture from the [Tiny Texture Pack](https://opengameart.org/content/tiny-texture-pack); rescaled, some desaturated or brightened to fit the palette; shipped by the Polis plugin, see `plugins/polis/public/CREDITS.md` | Screaming Brain Studios | CC0-1.0 | 9CDD6578BA4572A3DCA9D03487A43604113A13839907FCAD5C551FEADE63D45D |

The three font files are WOFF2 variable Latin normal subsets. The corresponding Fontsource package pages identify all three exact 5.3.0 packages as OFL-1.1, and the upstream font copyright notices are recorded above. Those Fontsource packages are not dependencies in the current package.json or pnpm-lock.yaml because the font binaries were copied into src/styles/fonts; preserve this attribution and the OFL text when redistributing them.

## SIL Open Font License 1.1

```text
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free
and open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The fonts,
including any derivative works, can be bundled, embedded, redistributed
and/or sold with any software provided that any reserved names are not used
by derivative works. The fonts and derivatives, however, cannot be released
under any other type of license. The requirement for fonts to remain under
this license does not apply to any document created using the fonts or their
derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may include
source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical writer or
other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font Software,
subject to the following conditions:

1) Neither the Font Software nor any of its individual components, in
Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy contains
the above copyright notice and this license. These can be included either as
stand-alone text files, human-readable headers or in the appropriate
machine-readable metadata fields within text or binary files as long as
those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font Name(s)
unless explicit written permission is granted by the corresponding Copyright
Holder. This restriction only applies to the primary font name as presented
to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any Modified
Version, except to acknowledge the contribution(s) or with their explicit
written permission.

5) The Font Software, modified or unmodified, in part or in whole, must be
distributed entirely under this license, and must not be distributed under
any other license. The requirement for fonts to remain under this license
does not apply to any document created using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM OTHER DEALINGS
IN THE FONT SOFTWARE.
```


## Direct linked dependencies

These are the dependencies declared directly by the workspace manifests or package.json. The complete resolved transitive inventory follows.

| Name | Version | Kind | Licence |
| --- | --- | --- | --- |
| alacritty_terminal | 0.26.0 | Rust direct optional server | Apache-2.0 |
| portable-pty | 0.9.0 | Rust direct optional server | MIT |
| rusqlite | 0.40.2 | Rust direct optional server; bundled SQLite | MIT |
| serde | 1.0.229 | Rust direct runtime | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | Rust direct runtime/test | MIT OR Apache-2.0 |
| tauri | 2.11.5 | Rust direct runtime | Apache-2.0 OR MIT |
| tauri-build | 2.6.3 | Rust direct build | Apache-2.0 OR MIT |
| windows-sys | 0.61.2 | Rust direct Windows | MIT OR Apache-2.0 |
| @tauri-apps/api | 2.11.1 | npm direct runtime | Apache-2.0 OR MIT |
| @tauri-apps/cli | 2.11.4 | npm direct build/test | Apache-2.0 OR MIT |
| @types/node | 26.4.0 | npm direct build/test | MIT |
| @types/react | 19.2.18 | npm direct build/test | MIT |
| @types/react-dom | 19.2.5 | npm direct build/test | MIT |
| @vitejs/plugin-react | 6.1.0 | npm direct build/test | MIT |
| @xterm/addon-fit | 0.11.0 | npm direct runtime | MIT |
| @xterm/xterm | 6.0.0 | npm direct runtime | MIT |
| oxfmt | 0.65.0 | npm direct build/test | MIT |
| oxlint | 1.80.0 | npm direct build/test | MIT |
| react | 19.2.8 | npm direct runtime | MIT |
| react-dom | 19.2.8 | npm direct runtime | MIT |
| typescript | 7.0.2 | npm direct build/test | Apache-2.0 |
| vite | 8.2.2 | npm direct build/test | MIT |
| vitest | 4.1.11 | npm direct build/test | MIT |
| zustand | 5.0.15 | npm direct runtime | MIT |

## Non-permissive, nonstandard, or unclear items

No package in the current Cargo or npm license metadata declares GPL or AGPL as its only or required licence. The graph is not an Apache/MIT/BSD-only graph, however:

- MPL-2.0 (weak copyleft): Rust cssparser 0.36.0, cssparser-macros 0.6.1, dtoa-short 0.3.5, option-ext 0.2.0, selectors 0.36.1; npm lightningcss 1.33.0, lightningcss-android-arm64 1.33.0, lightningcss-darwin-arm64 1.33.0, lightningcss-darwin-x64 1.33.0, lightningcss-freebsd-x64 1.33.0, lightningcss-linux-arm-gnueabihf 1.33.0, lightningcss-linux-arm64-gnu 1.33.0, lightningcss-linux-arm64-musl 1.33.0, lightningcss-linux-x64-gnu 1.33.0, lightningcss-linux-x64-musl 1.33.0, lightningcss-win32-arm64-msvc 1.33.0, lightningcss-win32-x64-msvc 1.33.0. Rust reachability is through Tauri tauri-utils/dom_query and dirs; npm reachability is through Vite. These are transitive, but MPL source/notice obligations still need to be respected if the covered code is redistributed.
- Removed since the previous inventory: npm caniuse-lite 1.0.30001810 (CC-BY-4.0, reached through the Browserslist compatibility-data chain) and npm minimatch 10.2.6 (BlueOak-1.0.0, reached through the removed ESLint tooling). Neither package appears in the current pnpm-lock.yaml.
- Unicode-3.0: Rust icu_collections 2.3.0, icu_locale_core 2.3.0, icu_normalizer 2.3.0, icu_normalizer_data 2.3.0, icu_properties 2.3.0, icu_properties_data 2.3.0, icu_provider 2.3.1, litemap 0.8.3, potential_utf 0.1.6, tinystr 0.8.4, writeable 0.6.4, yoke 0.8.3, yoke-derive 0.8.2, zerofrom 0.1.8, zerofrom-derive 0.1.7, zerotrie 0.2.5, zerovec 0.11.8, zerovec-derive 0.11.6. These Unicode data/library crates are permissive but carry a separate notice regime; keep the declared licence instead of normalizing it to MIT.
- LGPL appears only as an option in r-efi 5.3.0 and 6.0.0 licence expressions: r-efi 5.3.0 (MIT OR Apache-2.0 OR LGPL-2.1-or-later), r-efi 6.0.0 (MIT OR Apache-2.0 OR LGPL-2.1-or-later). The same expressions also offer MIT or Apache-2.0; confirm the selected downstream option in any binary notice process.
- Unlicense appears in Rust dual/alternative expressions including aho-corasick 1.1.5, byteorder 1.5.0, jiff 0.2.35, jiff-core 0.1.0, jiff-static 0.2.35, jiff-tzdb 0.1.8, jiff-tzdb-platform 0.1.3, memchr 2.8.3, same-file 1.0.6, walkdir 2.5.0, winapi-util 0.1.11. This is not copyleft, but the project should retain the exact expression because the legal treatment of public-domain dedications can vary by jurisdiction.

No unresolved asset provenance remains: the icon is original project artwork carrying this project's own Apache-2.0 licence (see the asset table above), and the fonts are attributed with package, upstream project, copyright, and licence.

## Complete resolved inventory

### Rust registry packages (Cargo.lock; 470 records)

| Name | Version | Kind | Licence |
| --- | --- | --- | --- |
| adler2 | 2.0.1 | Rust transitive (lockfile) | 0BSD OR MIT OR Apache-2.0 |
| aho-corasick | 1.1.5 | Rust transitive (lockfile) | Unlicense OR MIT |
| alacritty_terminal | 0.26.0 | Rust direct optional server | Apache-2.0 |
| alloc-no-stdlib | 2.0.4 | Rust transitive (lockfile) | BSD-3-Clause |
| alloc-stdlib | 0.2.4 | Rust transitive (lockfile) | BSD-3-Clause |
| android_system_properties | 0.1.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| anyhow | 1.0.104 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| arrayvec | 0.7.8 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| atk | 0.18.2 | Rust transitive (lockfile) | MIT |
| atk-sys | 0.18.2 | Rust transitive (lockfile) | MIT |
| atomic-waker | 1.1.2 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| autocfg | 1.5.1 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| base64 | 0.21.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| base64 | 0.22.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| bit-set | 0.8.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| bit-vec | 0.8.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| bitflags | 1.3.2 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| bitflags | 2.13.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| block-buffer | 0.10.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| block2 | 0.6.2 | Rust transitive (lockfile) | MIT |
| brotli | 8.0.4 | Rust transitive (lockfile) | BSD-3-Clause AND MIT |
| brotli-decompressor | 5.0.3 | Rust transitive (lockfile) | BSD-3-Clause/MIT |
| bs58 | 0.5.1 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| bumpalo | 3.20.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| bytemuck | 1.25.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| byteorder | 1.5.0 | Rust transitive (lockfile) | Unlicense OR MIT |
| bytes | 1.12.1 | Rust transitive (lockfile) | MIT |
| cairo-rs | 0.18.5 | Rust transitive (lockfile) | MIT |
| cairo-sys-rs | 0.18.2 | Rust transitive (lockfile) | MIT |
| camino | 1.2.5 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| cargo_metadata | 0.19.2 | Rust transitive (lockfile) | MIT |
| cargo_toml | 0.22.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| cargo-platform | 0.1.9 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| cc | 1.4.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| cesu8 | 1.1.0 | Rust transitive (lockfile) | Apache-2.0/MIT |
| cfb | 0.7.3 | Rust transitive (lockfile) | MIT |
| cfg_aliases | 0.1.1 | Rust transitive (lockfile) | MIT |
| cfg-expr | 0.15.8 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| cfg-if | 1.0.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| chrono | 0.4.45 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| combine | 4.6.8 | Rust transitive (lockfile) | MIT |
| concurrent-queue | 2.5.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| cookie | 0.18.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| core-foundation | 0.10.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| core-foundation-sys | 0.8.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| core-graphics | 0.25.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| core-graphics-types | 0.2.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| cpufeatures | 0.2.17 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| crc32fast | 1.5.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| crossbeam-channel | 0.5.16 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| crossbeam-utils | 0.8.22 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| crypto-common | 0.1.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| cssparser | 0.36.0 | Rust transitive (lockfile) | MPL-2.0 |
| cssparser-macros | 0.6.1 | Rust transitive (lockfile) | MPL-2.0 |
| ctor | 0.8.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| ctor-proc-macro | 0.0.7 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| cursor-icon | 1.2.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 OR Zlib |
| darling | 0.23.0 | Rust transitive (lockfile) | MIT |
| darling_core | 0.23.0 | Rust transitive (lockfile) | MIT |
| darling_macro | 0.23.0 | Rust transitive (lockfile) | MIT |
| dbus | 0.9.12 | Rust transitive (lockfile) | Apache-2.0/MIT |
| defmt | 1.1.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| defmt-macros | 1.1.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| defmt-parser | 1.0.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| deranged | 0.5.8 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| derive_more | 2.1.1 | Rust transitive (lockfile) | MIT |
| derive_more-impl | 2.1.1 | Rust transitive (lockfile) | MIT |
| digest | 0.10.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| dirs | 6.0.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| dirs-sys | 0.5.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| dispatch2 | 0.3.1 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| displaydoc | 0.2.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| dlopen2 | 0.8.2 | Rust transitive (lockfile) | MIT |
| dlopen2_derive | 0.4.3 | Rust transitive (lockfile) | MIT |
| dom_query | 0.27.0 | Rust transitive (lockfile) | MIT |
| downcast-rs | 1.2.1 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| dpi | 0.1.2 | Rust transitive (lockfile) | Apache-2.0 AND MIT |
| dtoa | 1.0.11 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| dtoa-short | 0.3.5 | Rust transitive (lockfile) | MPL-2.0 |
| dtor | 0.3.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| dtor-proc-macro | 0.0.6 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| dunce | 1.0.5 | Rust transitive (lockfile) | CC0-1.0 OR MIT-0 OR Apache-2.0 |
| dyn-clone | 1.0.20 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| embed_plist | 1.2.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| embed-resource | 3.0.11 | Rust transitive (lockfile) | MIT |
| equivalent | 1.0.2 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| erased-serde | 0.4.10 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| errno | 0.3.14 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| fallible-iterator | 0.3.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| fallible-streaming-iterator | 0.1.9 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| fastrand | 2.5.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| fdeflate | 0.3.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| field-offset | 0.3.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| filedescriptor | 0.8.3 | Rust transitive (lockfile) | MIT |
| find-msvc-tools | 0.1.11 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| flate2 | 1.1.9 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| fnv | 1.0.7 | Rust transitive (lockfile) | Apache-2.0 / MIT |
| foldhash | 0.2.0 | Rust transitive (lockfile) | Zlib |
| foreign-types | 0.5.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| foreign-types-macros | 0.2.4 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| foreign-types-shared | 0.3.1 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| form_urlencoded | 1.2.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-channel | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-core | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-executor | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-io | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-macro | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-sink | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-task | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| futures-util | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| gdk | 0.18.2 | Rust transitive (lockfile) | MIT |
| gdk-pixbuf | 0.18.5 | Rust transitive (lockfile) | MIT |
| gdk-pixbuf-sys | 0.18.0 | Rust transitive (lockfile) | MIT |
| gdk-sys | 0.18.2 | Rust transitive (lockfile) | MIT |
| gdkwayland-sys | 0.18.2 | Rust transitive (lockfile) | MIT |
| gdkx11 | 0.18.2 | Rust transitive (lockfile) | MIT |
| gdkx11-sys | 0.18.2 | Rust transitive (lockfile) | MIT |
| generic-array | 0.14.7 | Rust transitive (lockfile) | MIT |
| getrandom | 0.2.17 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| getrandom | 0.3.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| getrandom | 0.4.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| gio | 0.18.4 | Rust transitive (lockfile) | MIT |
| gio-sys | 0.18.1 | Rust transitive (lockfile) | MIT |
| glib | 0.18.5 | Rust transitive (lockfile) | MIT |
| glib-macros | 0.18.5 | Rust transitive (lockfile) | MIT |
| glib-sys | 0.18.1 | Rust transitive (lockfile) | MIT |
| glob | 0.3.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| gobject-sys | 0.18.0 | Rust transitive (lockfile) | MIT |
| gtk | 0.18.2 | Rust transitive (lockfile) | MIT |
| gtk-sys | 0.18.2 | Rust transitive (lockfile) | MIT |
| gtk3-macros | 0.18.2 | Rust transitive (lockfile) | MIT |
| hashbrown | 0.12.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| hashbrown | 0.16.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| hashbrown | 0.17.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| hashlink | 0.12.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| heck | 0.4.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| heck | 0.5.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| hermit-abi | 0.5.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| hex | 0.4.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| home | 0.5.12 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| html5ever | 0.38.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| http | 1.5.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| http-body | 1.1.0 | Rust transitive (lockfile) | MIT |
| http-body-util | 0.1.5 | Rust transitive (lockfile) | MIT |
| httparse | 1.10.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| hyper | 1.11.0 | Rust transitive (lockfile) | MIT |
| hyper-util | 0.1.20 | Rust transitive (lockfile) | MIT |
| iana-time-zone | 0.1.65 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| iana-time-zone-haiku | 0.1.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| ico | 0.5.0 | Rust transitive (lockfile) | MIT |
| icu_collections | 2.3.0 | Rust transitive (lockfile) | Unicode-3.0 |
| icu_locale_core | 2.3.0 | Rust transitive (lockfile) | Unicode-3.0 |
| icu_normalizer | 2.3.0 | Rust transitive (lockfile) | Unicode-3.0 |
| icu_normalizer_data | 2.3.0 | Rust transitive (lockfile) | Unicode-3.0 |
| icu_properties | 2.3.0 | Rust transitive (lockfile) | Unicode-3.0 |
| icu_properties_data | 2.3.0 | Rust transitive (lockfile) | Unicode-3.0 |
| icu_provider | 2.3.1 | Rust transitive (lockfile) | Unicode-3.0 |
| ident_case | 1.0.1 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| idna | 1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| idna_adapter | 1.2.2 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| indexmap | 1.9.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| indexmap | 2.14.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| infer | 0.19.0 | Rust transitive (lockfile) | MIT |
| ipnet | 2.12.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| itoa | 1.0.18 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| javascriptcore-rs | 1.1.2 | Rust transitive (lockfile) | MIT |
| javascriptcore-rs-sys | 1.1.1 | Rust transitive (lockfile) | MIT |
| jiff | 0.2.35 | Rust transitive (lockfile) | Unlicense OR MIT |
| jiff-core | 0.1.0 | Rust transitive (lockfile) | Unlicense OR MIT |
| jiff-static | 0.2.35 | Rust transitive (lockfile) | Unlicense OR MIT |
| jiff-tzdb | 0.1.8 | Rust transitive (lockfile) | Unlicense OR MIT |
| jiff-tzdb-platform | 0.1.3 | Rust transitive (lockfile) | Unlicense OR MIT |
| jni | 0.21.1 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| jni-sys | 0.3.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| jni-sys | 0.4.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| jni-sys-macros | 0.4.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| js-sys | 0.3.104 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| json-patch | 3.0.1 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| json5 | 0.4.1 | Rust transitive (lockfile) | ISC |
| jsonptr | 0.6.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| keyboard-types | 0.7.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| lazy_static | 1.5.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| libappindicator | 0.9.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| libappindicator-sys | 0.9.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| libc | 0.2.189 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| libdbus-sys | 0.2.7 | Rust transitive (lockfile) | Apache-2.0/MIT |
| libloading | 0.7.4 | Rust transitive (lockfile) | ISC |
| libredox | 0.1.20 | Rust transitive (lockfile) | MIT |
| libsqlite3-sys | 0.38.2 | Rust transitive (lockfile) | MIT |
| linux-raw-sys | 0.12.1 | Rust transitive (lockfile) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| litemap | 0.8.3 | Rust transitive (lockfile) | Unicode-3.0 |
| lock_api | 0.4.14 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| log | 0.4.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| markup5ever | 0.38.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| memchr | 2.8.3 | Rust transitive (lockfile) | Unlicense OR MIT |
| memoffset | 0.9.1 | Rust transitive (lockfile) | MIT |
| mime | 0.3.17 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| miniz_oxide | 0.8.9 | Rust transitive (lockfile) | MIT OR Zlib OR Apache-2.0 |
| mio | 1.2.2 | Rust transitive (lockfile) | MIT |
| miow | 0.6.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| muda | 0.19.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| ndk | 0.9.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| ndk-sys | 0.6.0+11769913 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| new_debug_unreachable | 1.0.6 | Rust transitive (lockfile) | MIT |
| nix | 0.28.0 | Rust transitive (lockfile) | MIT |
| num_enum | 0.7.6 | Rust transitive (lockfile) | BSD-3-Clause OR MIT OR Apache-2.0 |
| num_enum_derive | 0.7.6 | Rust transitive (lockfile) | BSD-3-Clause OR MIT OR Apache-2.0 |
| num-conv | 0.2.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| num-traits | 0.2.19 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| objc2 | 0.6.4 | Rust transitive (lockfile) | MIT |
| objc2-app-kit | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-cloud-kit | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-core-data | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-core-foundation | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-core-graphics | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-core-image | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-core-location | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-core-text | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-encode | 4.1.0 | Rust transitive (lockfile) | MIT |
| objc2-exception-helper | 0.1.1 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-foundation | 0.3.2 | Rust transitive (lockfile) | MIT |
| objc2-io-surface | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-quartz-core | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-ui-kit | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-user-notifications | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| objc2-web-kit | 0.3.2 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| once_cell | 1.21.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| option-ext | 0.2.0 | Rust transitive (lockfile) | MPL-2.0 |
| pango | 0.18.3 | Rust transitive (lockfile) | MIT |
| pango-sys | 0.18.0 | Rust transitive (lockfile) | MIT |
| parking_lot | 0.12.5 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| parking_lot_core | 0.9.12 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| percent-encoding | 2.3.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| pest | 2.9.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| pest_derive | 2.9.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| pest_generator | 2.9.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| pest_meta | 2.9.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| phf | 0.13.1 | Rust transitive (lockfile) | MIT |
| phf_codegen | 0.13.1 | Rust transitive (lockfile) | MIT |
| phf_generator | 0.13.1 | Rust transitive (lockfile) | MIT |
| phf_macros | 0.13.1 | Rust transitive (lockfile) | MIT |
| phf_shared | 0.13.1 | Rust transitive (lockfile) | MIT |
| pin-project-lite | 0.2.17 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| piper | 0.2.5 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| pkg-config | 0.3.34 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| plist | 1.10.0 | Rust transitive (lockfile) | MIT |
| png | 0.17.16 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| png | 0.18.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| polling | 3.11.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| portable-atomic | 1.15.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| portable-atomic-util | 0.2.7 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| portable-pty | 0.9.0 | Rust direct optional server | MIT |
| potential_utf | 0.1.6 | Rust transitive (lockfile) | Unicode-3.0 |
| powerfmt | 0.2.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| precomputed-hash | 0.1.1 | Rust transitive (lockfile) | MIT |
| proc-macro-crate | 1.3.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| proc-macro-crate | 2.0.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| proc-macro-crate | 3.5.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| proc-macro-error | 1.0.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| proc-macro-error-attr | 1.0.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| proc-macro2 | 1.0.107 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| quick-xml | 0.41.0 | Rust transitive (lockfile) | MIT |
| quote | 1.0.47 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| r-efi | 5.3.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| r-efi | 6.0.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 OR LGPL-2.1-or-later |
| raw-window-handle | 0.6.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 OR Zlib |
| redox_syscall | 0.5.18 | Rust transitive (lockfile) | MIT |
| redox_users | 0.5.2 | Rust transitive (lockfile) | MIT |
| ref-cast | 1.0.27 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| ref-cast-impl | 1.0.27 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| regex | 1.13.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| regex-automata | 0.4.18 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| regex-syntax | 0.8.11 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| reqwest | 0.13.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| rsqlite-vfs | 0.1.1 | Rust transitive (lockfile) | MIT |
| rusqlite | 0.40.2 | Rust direct optional server; bundled SQLite | MIT |
| rustc_version | 0.4.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| rustc-hash | 2.1.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| rustix | 1.1.4 | Rust transitive (lockfile) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustix-openpty | 0.2.0 | Rust transitive (lockfile) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| rustversion | 1.0.23 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| same-file | 1.0.6 | Rust transitive (lockfile) | Unlicense/MIT |
| schemars | 0.8.22 | Rust transitive (lockfile) | MIT |
| schemars | 0.9.0 | Rust transitive (lockfile) | MIT |
| schemars | 1.2.2 | Rust transitive (lockfile) | MIT |
| schemars_derive | 0.8.22 | Rust transitive (lockfile) | MIT |
| scopeguard | 1.2.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| selectors | 0.36.1 | Rust transitive (lockfile) | MPL-2.0 |
| semver | 1.0.28 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde | 1.0.229 | Rust direct runtime | MIT OR Apache-2.0 |
| serde_core | 1.0.229 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde_derive | 1.0.229 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde_derive_internals | 0.29.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde_json | 1.0.151 | Rust direct runtime/test | MIT OR Apache-2.0 |
| serde_repr | 0.1.21 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde_spanned | 0.6.9 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde_spanned | 1.1.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde_with | 3.22.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde_with_macros | 3.22.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serde-untagged | 0.1.9 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serial2 | 0.2.38 | Rust transitive (lockfile) | BSD-2-Clause OR Apache-2.0 |
| serialize-to-javascript | 0.1.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| serialize-to-javascript-impl | 0.1.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| servo_arc | 0.4.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| sha2 | 0.10.9 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| shared_library | 0.1.9 | Rust transitive (lockfile) | Apache-2.0/MIT |
| shell-words | 1.1.1 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| shlex | 2.0.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| signal-hook | 0.4.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| signal-hook-registry | 1.4.8 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| simd-adler32 | 0.3.10 | Rust transitive (lockfile) | MIT |
| siphasher | 1.0.3 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| slab | 0.4.12 | Rust transitive (lockfile) | MIT |
| smallvec | 1.15.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| socket2 | 0.6.5 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| softbuffer | 0.4.8 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| soup3 | 0.5.0 | Rust transitive (lockfile) | MIT |
| soup3-sys | 0.5.0 | Rust transitive (lockfile) | MIT |
| sqlite-wasm-rs | 0.5.5 | Rust transitive (lockfile) | MIT |
| stable_deref_trait | 1.2.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| string_cache | 0.9.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| string_cache_codegen | 0.6.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| strsim | 0.11.1 | Rust transitive (lockfile) | MIT |
| swift-rs | 1.0.8 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| syn | 1.0.109 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| syn | 2.0.119 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| syn | 3.0.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| sync_wrapper | 1.0.2 | Rust transitive (lockfile) | Apache-2.0 |
| synstructure | 0.13.2 | Rust transitive (lockfile) | MIT |
| system-deps | 6.2.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| tao | 0.35.3 | Rust transitive (lockfile) | Apache-2.0 |
| tao-macros | 0.1.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| target-lexicon | 0.12.16 | Rust transitive (lockfile) | Apache-2.0 WITH LLVM-exception |
| tauri | 2.11.5 | Rust direct runtime | Apache-2.0 OR MIT |
| tauri-build | 2.6.3 | Rust direct build | Apache-2.0 OR MIT |
| tauri-codegen | 2.6.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| tauri-macros | 2.6.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| tauri-runtime | 2.11.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| tauri-runtime-wry | 2.11.4 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| tauri-utils | 2.9.3 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| tauri-winres | 0.3.6 | Rust transitive (lockfile) | MIT |
| tendril | 0.5.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| thiserror | 1.0.69 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| thiserror | 2.0.20 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| thiserror-impl | 1.0.69 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| thiserror-impl | 2.0.20 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| time | 0.3.55 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| time-core | 0.1.9 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| time-macros | 0.2.32 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| tinystr | 0.8.4 | Rust transitive (lockfile) | Unicode-3.0 |
| tinyvec | 1.12.0 | Rust transitive (lockfile) | Zlib OR Apache-2.0 OR MIT |
| tinyvec_macros | 0.1.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 OR Zlib |
| tokio | 1.53.1 | Rust transitive (lockfile) | MIT |
| tokio-util | 0.7.19 | Rust transitive (lockfile) | MIT |
| toml | 0.8.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml | 0.9.12+spec-1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml | 1.1.4+spec-1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_datetime | 0.6.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_datetime | 0.7.5+spec-1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_datetime | 1.1.1+spec-1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_edit | 0.19.15 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_edit | 0.20.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_edit | 0.25.13+spec-1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_parser | 1.1.3+spec-1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| toml_writer | 1.1.2+spec-1.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| tower | 0.5.3 | Rust transitive (lockfile) | MIT |
| tower-http | 0.6.11 | Rust transitive (lockfile) | MIT |
| tower-layer | 0.3.3 | Rust transitive (lockfile) | MIT |
| tower-service | 0.3.3 | Rust transitive (lockfile) | MIT |
| tracing | 0.1.44 | Rust transitive (lockfile) | MIT |
| tracing-core | 0.1.36 | Rust transitive (lockfile) | MIT |
| tray-icon | 0.24.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| try-lock | 0.2.5 | Rust transitive (lockfile) | MIT |
| typeid | 1.0.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| typenum | 1.20.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| ucd-trie | 0.1.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| unic-char-property | 0.9.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| unic-char-range | 0.9.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| unic-common | 0.9.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| unic-ucd-ident | 0.9.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| unic-ucd-version | 0.9.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| unicode-ident | 1.0.24 | Rust transitive (lockfile) | (MIT OR Apache-2.0) AND Unicode-3.0 |
| unicode-segmentation | 1.13.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| unicode-width | 0.2.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| url | 2.5.8 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| urlpattern | 0.3.0 | Rust transitive (lockfile) | MIT |
| utf8_iter | 1.0.4 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| uuid | 1.25.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| vcpkg | 0.2.15 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| version_check | 0.9.5 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| version-compare | 0.2.1 | Rust transitive (lockfile) | MIT |
| vswhom | 0.1.0 | Rust transitive (lockfile) | MIT |
| vswhom-sys | 0.1.3 | Rust transitive (lockfile) | MIT |
| vte | 0.15.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| walkdir | 2.5.0 | Rust transitive (lockfile) | Unlicense/MIT |
| want | 0.3.1 | Rust transitive (lockfile) | MIT |
| wasi | 0.11.1+wasi-snapshot-preview1 | Rust transitive (lockfile) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasip2 | 1.0.4+wasi-0.2.12 | Rust transitive (lockfile) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| wasm-bindgen | 0.2.127 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| wasm-bindgen-futures | 0.4.77 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| wasm-bindgen-macro | 0.2.127 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| wasm-bindgen-macro-support | 0.2.127 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| wasm-bindgen-shared | 0.2.127 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| wasm-streams | 0.5.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| web_atoms | 0.2.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| web-sys | 0.3.104 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| webkit2gtk | 2.0.2 | Rust transitive (lockfile) | MIT |
| webkit2gtk-sys | 2.0.2 | Rust transitive (lockfile) | MIT |
| webview2-com | 0.38.2 | Rust transitive (lockfile) | MIT |
| webview2-com-macros | 0.8.1 | Rust transitive (lockfile) | MIT |
| webview2-com-sys | 0.38.2 | Rust transitive (lockfile) | MIT |
| winapi | 0.3.9 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| winapi-i686-pc-windows-gnu | 0.4.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| winapi-util | 0.1.11 | Rust transitive (lockfile) | Unlicense OR MIT |
| winapi-x86_64-pc-windows-gnu | 0.4.0 | Rust transitive (lockfile) | MIT/Apache-2.0 |
| window-vibrancy | 0.6.0 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| windows | 0.61.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_aarch64_gnullvm | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_aarch64_msvc | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_i686_gnu | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_i686_gnullvm | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_i686_msvc | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_x86_64_gnu | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_x86_64_gnullvm | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows_x86_64_msvc | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-collections | 0.2.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-core | 0.61.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-core | 0.62.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-future | 0.2.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-implement | 0.60.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-interface | 0.59.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-link | 0.1.3 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-link | 0.2.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-numerics | 0.2.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-result | 0.3.4 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-result | 0.4.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-strings | 0.4.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-strings | 0.5.1 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-sys | 0.45.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-sys | 0.59.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-sys | 0.61.2 | Rust direct Windows | MIT OR Apache-2.0 |
| windows-targets | 0.42.2 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-targets | 0.52.6 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-threading | 0.1.0 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| windows-version | 0.1.7 | Rust transitive (lockfile) | MIT OR Apache-2.0 |
| winnow | 0.5.40 | Rust transitive (lockfile) | MIT |
| winnow | 0.7.15 | Rust transitive (lockfile) | MIT |
| winnow | 1.0.4 | Rust transitive (lockfile) | MIT |
| winreg | 0.10.1 | Rust transitive (lockfile) | MIT |
| winreg | 0.55.0 | Rust transitive (lockfile) | MIT |
| wit-bindgen | 0.57.1 | Rust transitive (lockfile) | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT |
| writeable | 0.6.4 | Rust transitive (lockfile) | Unicode-3.0 |
| wry | 0.55.1 | Rust transitive (lockfile) | Apache-2.0 OR MIT |
| x11 | 2.21.0 | Rust transitive (lockfile) | MIT |
| x11-dl | 2.21.0 | Rust transitive (lockfile) | MIT |
| yoke | 0.8.3 | Rust transitive (lockfile) | Unicode-3.0 |
| yoke-derive | 0.8.2 | Rust transitive (lockfile) | Unicode-3.0 |
| zerofrom | 0.1.8 | Rust transitive (lockfile) | Unicode-3.0 |
| zerofrom-derive | 0.1.7 | Rust transitive (lockfile) | Unicode-3.0 |
| zerotrie | 0.2.5 | Rust transitive (lockfile) | Unicode-3.0 |
| zerovec | 0.11.8 | Rust transitive (lockfile) | Unicode-3.0 |
| zerovec-derive | 0.11.6 | Rust transitive (lockfile) | Unicode-3.0 |
| zmij | 1.0.23 | Rust transitive (lockfile) | MIT |

### npm packages (pnpm-lock.yaml; 156 records)

| Name | Version | Kind | Licence |
| --- | --- | --- | --- |
| @jridgewell/sourcemap-codec | 1.5.5 | npm transitive | MIT |
| @oxc-project/types | 0.147.0 | npm transitive | MIT |
| @oxfmt/binding-android-arm-eabi | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-android-arm64 | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-darwin-arm64 | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-darwin-x64 | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-freebsd-x64 | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-arm-gnueabihf | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-arm-musleabihf | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-arm64-gnu | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-arm64-musl | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-ppc64-gnu | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-riscv64-gnu | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-riscv64-musl | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-s390x-gnu | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-x64-gnu | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-linux-x64-musl | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-openharmony-arm64 | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-win32-arm64-msvc | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-win32-ia32-msvc | 0.65.0 | npm transitive optional/platform | MIT |
| @oxfmt/binding-win32-x64-msvc | 0.65.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-android-arm-eabi | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-android-arm64 | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-darwin-arm64 | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-darwin-x64 | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-freebsd-x64 | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-arm-gnueabihf | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-arm-musleabihf | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-arm64-gnu | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-arm64-musl | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-ppc64-gnu | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-riscv64-gnu | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-riscv64-musl | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-s390x-gnu | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-x64-gnu | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-linux-x64-musl | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-openharmony-arm64 | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-win32-arm64-msvc | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-win32-ia32-msvc | 1.80.0 | npm transitive optional/platform | MIT |
| @oxlint/binding-win32-x64-msvc | 1.80.0 | npm transitive optional/platform | MIT |
| @rolldown/binding-android-arm-eabi | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-android-arm64 | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-darwin-arm64 | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-darwin-x64 | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-freebsd-x64 | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-linux-arm-gnueabihf | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-linux-arm64-gnu | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-linux-arm64-musl | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-linux-ppc64-gnu | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-linux-s390x-gnu | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-linux-x64-gnu | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-linux-x64-musl | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-openharmony-arm64 | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-win32-arm64-msvc | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/binding-win32-x64-msvc | 1.2.6 | npm transitive optional/platform | MIT |
| @rolldown/pluginutils | 1.0.1 | npm transitive | MIT |
| @standard-schema/spec | 1.1.0 | npm transitive | MIT |
| @tauri-apps/api | 2.11.1 | npm direct runtime | Apache-2.0 OR MIT |
| @tauri-apps/cli | 2.11.4 | npm direct build/test | Apache-2.0 OR MIT |
| @tauri-apps/cli-darwin-arm64 | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-darwin-x64 | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-linux-arm-gnueabihf | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-linux-arm64-gnu | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-linux-arm64-musl | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-linux-riscv64-gnu | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-linux-x64-gnu | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-linux-x64-musl | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-win32-arm64-msvc | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-win32-ia32-msvc | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @tauri-apps/cli-win32-x64-msvc | 2.11.4 | npm transitive optional/platform | Apache-2.0 OR MIT |
| @types/chai | 5.2.3 | npm transitive | MIT |
| @types/deep-eql | 4.0.2 | npm transitive | MIT |
| @types/estree | 1.0.9 | npm transitive | MIT |
| @types/node | 26.4.0 | npm direct build/test | MIT |
| @types/react | 19.2.18 | npm direct build/test | MIT |
| @types/react-dom | 19.2.5 | npm direct build/test | MIT |
| @typescript/typescript-aix-ppc64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-darwin-arm64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-darwin-x64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-freebsd-arm64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-freebsd-x64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-arm | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-arm64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-loong64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-mips64el | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-ppc64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-riscv64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-s390x | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-linux-x64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-netbsd-arm64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-netbsd-x64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-openbsd-arm64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-openbsd-x64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-sunos-x64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-win32-arm64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @typescript/typescript-win32-x64 | 7.0.2 | npm transitive optional/platform | Apache-2.0 |
| @vitejs/plugin-react | 6.1.0 | npm direct build/test | MIT |
| @vitest/expect | 4.1.11 | npm transitive | MIT |
| @vitest/mocker | 4.1.11 | npm transitive | MIT |
| @vitest/pretty-format | 4.1.11 | npm transitive | MIT |
| @vitest/runner | 4.1.11 | npm transitive | MIT |
| @vitest/snapshot | 4.1.11 | npm transitive | MIT |
| @vitest/spy | 4.1.11 | npm transitive | MIT |
| @vitest/utils | 4.1.11 | npm transitive | MIT |
| @xterm/addon-fit | 0.11.0 | npm direct runtime | MIT |
| @xterm/xterm | 6.0.0 | npm direct runtime | MIT |
| assertion-error | 2.0.1 | npm transitive | MIT |
| chai | 6.2.2 | npm transitive | MIT |
| convert-source-map | 2.0.0 | npm transitive | MIT |
| csstype | 3.2.3 | npm transitive | MIT |
| detect-libc | 2.1.2 | npm transitive | Apache-2.0 |
| es-module-lexer | 2.3.2 | npm transitive | MIT |
| estree-walker | 3.0.3 | npm transitive | MIT |
| expect-type | 1.4.0 | npm transitive | Apache-2.0 |
| fdir | 6.5.0 | npm transitive | MIT |
| fsevents | 2.3.3 | npm transitive optional/platform | MIT |
| lightningcss | 1.33.0 | npm transitive | MPL-2.0 |
| lightningcss-android-arm64 | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-darwin-arm64 | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-darwin-x64 | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-freebsd-x64 | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-linux-arm-gnueabihf | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-linux-arm64-gnu | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-linux-arm64-musl | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-linux-x64-gnu | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-linux-x64-musl | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-win32-arm64-msvc | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| lightningcss-win32-x64-msvc | 1.33.0 | npm transitive optional/platform | MPL-2.0 |
| magic-string | 0.30.21 | npm transitive | MIT |
| nanoid | 3.3.18 | npm transitive | MIT |
| obug | 2.1.4 | npm transitive | MIT |
| oxfmt | 0.65.0 | npm direct build/test | MIT |
| oxlint | 1.80.0 | npm direct build/test | MIT |
| pathe | 2.0.3 | npm transitive | MIT |
| picocolors | 1.1.1 | npm transitive | ISC |
| picomatch | 4.0.7 | npm transitive | MIT |
| postcss | 8.5.26 | npm transitive | MIT |
| react | 19.2.8 | npm direct runtime | MIT |
| react-dom | 19.2.8 | npm direct runtime | MIT |
| rolldown | 1.2.6 | npm transitive | MIT |
| scheduler | 0.27.0 | npm transitive | MIT |
| siginfo | 2.0.0 | npm transitive | ISC |
| source-map-js | 1.2.1 | npm transitive | BSD-3-Clause |
| stackback | 0.0.2 | npm transitive | MIT |
| std-env | 4.2.0 | npm transitive | MIT |
| tinybench | 2.9.0 | npm transitive | MIT |
| tinyexec | 1.3.0 | npm transitive | MIT |
| tinyglobby | 0.2.17 | npm transitive | MIT |
| tinypool | 2.1.0 | npm transitive | MIT |
| tinyrainbow | 3.1.1 | npm transitive | MIT |
| typescript | 7.0.2 | npm direct build/test | Apache-2.0 |
| undici-types | 8.3.0 | npm transitive | MIT |
| vite | 8.2.2 | npm direct build/test | MIT |
| vitest | 4.1.11 | npm direct build/test | MIT |
| why-is-node-running | 2.3.0 | npm transitive | MIT |
| zustand | 5.0.15 | npm direct runtime | MIT |

## License text and redistribution

The repository LICENSE and NOTICE cover Devboule project code. This file identifies the third-party package and asset licences; it does not relicense them. For a binary release, retain the relevant package license/notice texts and the attribution requirements identified above alongside the application notice. In particular, do not omit MPL-2.0, Unicode-3.0, or OFL-1.1 material just because the package is transitive or the asset is loaded by a build step.

Useful canonical licence texts: Apache-2.0 https://www.apache.org/licenses/LICENSE-2.0, MPL-2.0 https://www.mozilla.org/en-US/MPL/2.0/, Unicode licence https://www.unicode.org/license.txt, and SIL OFL 1.1 https://scripts.sil.org/OFL.
