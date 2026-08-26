# M1a — report di consegna

Data: 26 agosto 2026
Repository: `Aspis0/devboule-v2`
Remote configurato: `https://github.com/Aspis0/devboule-v2`
Push eseguito: no

## Cosa è stato creato

- Scaffold Tauri v2 + React + TypeScript strict + Vite con `pnpm`.
- React e ReactDOM bloccati a `18.3.1`; Zustand `5.0.15` è l’unico store globale.
- Nessun router, Tailwind o CSS-in-JS.
- Workspace Cargo con `src-tauri` come membro e `crates/README.md` come segnaposto per
  `devboule-daemon` e `oracle-core`.
- `src/lib/tauri.ts` come unico confine frontend per `invoke` e `Channel`, con wrapper
  tipizzati per il protocollo iniziale di sessioni, provider e Oracle.
- Guscio UI M1a: finestra minima 1280px, palette in variabili CSS, font OFL locali,
  sei superfici placeholder e nav a mezzaluna.
- Mezzaluna con cinque punti navigabili nelle coordinate del design, peek zone da 110px,
  sliver da 13px, `pageShift`/`pageDim`, apertura in hover o tastiera, chiusura oltre 150px
  o all’uscita dalla finestra. Oracle è raggiungibile dalla tab Oracle dentro Settings.
- `LICENSE`, `NOTICE`, `THIRD_PARTY.md`, Dependabot, CI Windows, toolchain pin, audit
  bloccanti e controllo automatico degli `npm-shrinkwrap.json`.
- Icona Windows copiata come asset in `src-tauri/icons/icon.ico`; `old-devboule` non è stato
  modificato.

## Correzioni del round review M1a

- B1: `pageShift` è `34px`, `pageDim` è `0.34`, la nav chiusa usa `translateY(-46px)`;
  le transizioni restano `transform 0.42s cubic-bezier(0.22, 0.9, 0.24, 1)` e
  `opacity 0.3s/0.32s ease` come nel design. Il guscio ha `z-index: 40`.
- B2: l’arco ha esattamente i cinque punti `workspace (277,44)`, `polis (371,80)`,
  `pubvia (470,92)`, `design (569,80)`, `settings (662,44)`, con margine `-14px 0 0 -14px`
  e area `28px × 28px`. Oracle non è una destinazione della mezzaluna: è una tab di Settings.
- B3: il file di eccezioni non operativo è stato rimosso. La versione pinnata di `cargo-audit`
  non legge quel formato di configurazione; lasciare un file ignorato avrebbe dato una falsa garanzia.
- B4: la mezzaluna è un landmark `navigation`, ha trigger focusabile, punti sempre nel tab
  order, focus ring visibile e `Escape` che chiude riportando il focus al trigger.
- B5: CSP aggiunta con `worker-src 'self' blob:` e WebSocket per il daemon su `localhost:6767`
  e `127.0.0.1:6767`. `unsafe-eval` non è presente; un commento nel punto `script-src` documenta
  che andrà valutato a M5 per PixiJS.
- B6: apertura, chiusura e hover della shell/nav usano esclusivamente Pointer Events.
- La configurazione Vite esclude `target/**` dal watcher, evitando la race Windows `EBUSY`
  mentre Cargo compila l’eseguibile dev.

## Componenti Tauri verificati

| Componente | Versione effettiva | Fonte |
| --- | ---: | --- |
| crate Rust `tauri` | `2.11.5` | `cargo tree -p devboule` |
| crate Rust `tauri-build` | `2.6.3` | `cargo tree -p devboule` |
| npm `@tauri-apps/cli` | `2.11.4` | `pnpm-lock.yaml` |
| npm `@tauri-apps/api` | `2.11.1` | `pnpm-lock.yaml` |

I crate Rust e i pacchetti npm seguono cicli di rilascio indipendenti; il commento è vicino
al pin in `src-tauri/Cargo.toml`. Inoltre, la verifica del registry ha dato:

```text
cargo info tauri@2.6.3
error: could not find `tauri@2.6.3` in registry `https://github.com/rust-lang/crates.io-index`

cargo info tauri-build@2.6.3
version: 2.6.3
```

Per questo il crate `tauri` è fissato all’ultima versione pubblicata disponibile (`2.11.5`),
mentre `tauri-build` resta fissato a `2.6.3`. Non è stato inserito un pin non installabile.

## Dipendenze aggiunte e motivazione

Runtime:

- `@tauri-apps/api@2.11.1` — unico accesso TS a IPC e Channel Tauri.
- `react@18.3.1` — runtime UI; React 18 è il vincolo deliberato per il futuro porting Polis.
- `react-dom@18.3.1` — renderer React per la webview.
- `zustand@5.0.15` — unico store globale per la superficie attiva.

Tooling:

- `@tauri-apps/cli@2.11.4` — comandi `tauri dev/build`.
- `@types/node@26.3.0` — tipi per configurazione Vite e script Node.
- `@types/react@18.3.31` — tipi compatibili con React 18.
- `@types/react-dom@18.3.7` — tipi compatibili con ReactDOM 18.
- `@vitejs/plugin-react@6.1.0` — trasformazione JSX React per Vite.
- `typescript@5.9.3` — type-checking strict separato dal bundling.
- `vite@8.2.2` — server di sviluppo e build frontend.

Rust:

- `tauri@2.11.5` — shell desktop Windows e registrazione comandi.
- `tauri-build@2.6.3` — build script Tauri; versione Rust indipendente dal CLI/API npm.
- Feature Rust `config-json5` su `tauri@2.11.5` e `tauri-build@2.6.3` — consente il commento
  CSP in `tauri.conf.json` mantenendo la configurazione validata dal parser ufficiale Tauri.
- Transitive Rust `json5@0.4.1`, `pest@2.9.0`, `pest_derive@2.9.0`, `pest_generator@2.9.0`,
  `pest_meta@2.9.0` e `ucd-trie@0.1.7` — parser JSON5 richiesto dalla feature precedente;
  non sono dipendenze applicative dirette.

Nessuna dipendenza npm è stata aggiunta nel round di correzione.

I font sono stati vendorizzati da tre pacchetti Fontsource Variable `5.3.0`, ma quei pacchetti
non sono dipendenze del progetto: solo i tre `.woff2` Latin normal sono nel repository. Le
licenze OFL sono in `THIRD_PARTY.md`.

## Verifica reale

Tutti i comandi sono stati eseguiti in `C:\Users\gualt\Desktop\New devboule\devboule-v2`.

### `pnpm install`

```text
Packages: +32
Done in 2.4s using pnpm v10.33.2
EXIT_CODE=0
ELAPSED_SECONDS=2.71
```

Installazione finale riproducibile:

```text
Lockfile is up to date, resolution step is skipped
Already up to date
Done in 681ms using pnpm v10.33.2
EXIT_CODE=0
ELAPSED_SECONDS=0.85
```

### `pnpm build`

```text
> devboule-v2@0.1.0 build C:\Users\gualt\Desktop\New devboule\devboule-v2
> tsc --noEmit && vite build

vite v8.2.2 building client environment for production...
✓ 22 modules transformed.
dist/index.html                                   0.44 kB │ gzip:  0.28 kB
dist/assets/Fraunces-Latin-ukD16Tqj.woff2        36.62 kB
dist/assets/JetBrainsMono-Latin-B9CIFXIH.woff2   40.40 kB
dist/assets/Inter-Latin-Dx4kXJAl.woff2           48.25 kB
dist/assets/index-CYT1Xgh_.css                    6.68 kB │ gzip:  2.10 kB
dist/assets/index-BPonuEwu.js                   146.87 kB │ gzip: 47.95 kB
✓ built in 218ms
DIST_FILES=6
DIST_BYTES=279284
EXIT_CODE=0
ELAPSED_SECONDS=3.26
```

Output finale `dist/`: 6 file, 279,284 byte totali. Il bundle principale è 146.87 kB
(47.95 kB gzip); i tre font risultano inclusi come asset locali.

### `cargo build` dentro `src-tauri`

```text
Compiling devboule v0.1.0 (C:\Users\gualt\Desktop\New devboule\devboule-v2\src-tauri)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.99s
DEBUG_EXE_BYTES=12300288
EXIT_CODE=0
ELAPSED_SECONDS=12.16
```

Artefatto: `target\debug\devboule.exe`, 12,300,288 byte.

### `pnpm tauri build --no-bundle`

```text
> devboule-v2@0.1.0 tauri
> tauri "build" "--no-bundle"

Running beforeBuildCommand `pnpm build`
✓ built in 222ms
Finished `release` profile [optimized] target(s) in 1m 07s
Built application at: C:\Users\gualt\Desktop\New devboule\devboule-v2\target\release\devboule.exe
EXIT_CODE=0
RELEASE_EXE_BYTES=8526848
ELAPSED_SECONDS=73.06
```

Artefatto release: `target\release\devboule.exe`, 8,526,848 byte.

### Controlli CI riprodotti localmente

```text
pnpm run check:shrinkwrap
No npm-shrinkwrap.json files found inside node_modules.
EXIT_CODE=0

pnpm audit --audit-level high
No known vulnerabilities found
EXIT_CODE=0

cargo clippy --workspace --all-targets --all-features -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.91s
EXIT_CODE=0

cargo audit
warning: 17 allowed warnings found
EXIT_CODE=0
```

`cargo audit` mostra 17 avvisi transitive `unmaintained`/`unsound` provenienti soprattutto
dallo stack GTK/Tauri e da crate Unicode; non ha trovato vulnerabilità bloccanti. Il tentativo
più severo `cargo audit --deny warnings` fallisce proprio su questi avvisi, quindi la CI usa
il comportamento standard: vulnerabilità effettive bloccanti, avvisi di manutenzione visibili.

### `pnpm tauri dev`

```text
Running BeforeDevCommand (`pnpm dev`)
VITE v8.2.2  ready in 384 ms
Local:   http://localhost:1420/
Running DevCommand (`cargo  run --no-default-features --color always --`)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.05s
Running `...\target\debug\devboule.exe`
PROCESS_STARTED=1; wrapper stopped with Ctrl+C (shell EXIT_CODE=1)
```

Il primo tentativo, prima dell’esclusione Vite di `target/**`, è terminato con `EBUSY` mentre
il watcher osservava `target\debug\deps\devboule.exe`; la configurazione è stata corretta e il
secondo avvio riportato sopra è riuscito. Il processo GUI è stato poi terminato manualmente.

## Non fatto / incompleto

- Le sei superfici sono placeholder: non sono stati portati Workspace, Polis, Oracle, Design,
  Pubvia o Settings reali.
- Nessun daemon, PTY, ACP/provider adapter, journal, Oracle runtime, atlas Polis o plugin loader.
- Nessuna chiamata IPC viene eseguita dalla UI M1a: il confine tipizzato è predisposto e il
  backend espone solo il comando minimale `app_identity` per il bootstrap.
- Nessun push GitHub è stato eseguito.
- `old-devboule` è stato usato solo in lettura; alla verifica finale il suo worktree risultava
  già non pulito con file marcati `D`, e non ho eseguito reset, checkout o scritture su quel tree.
