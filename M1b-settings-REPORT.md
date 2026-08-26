# M1b — Settings report

Date: 26 August 2026  
Repository: `devboule-v2`  
Push: no

## Built

- Added the Settings surface with the six design tabs: General, Projects,
  Oracle, Providers & models, Devices, and Labs.
- Kept Oracle inside Settings. The five-point crescent navigation was not
  changed and still has no Oracle destination.
- Added the Providers & models view with the four design providers, exact
  details, semantic statuses, keyboard-accessible switches, and the two
  default-model cards. Mock provider discovery keeps `installed` and
  `authenticated` separate; PATH presence is never treated as authentication.
- Added the Oracle administration panel with the design health row, six doctor
  checks and tooltips, index statistics, watcher controls, indexing progress,
  Indexed/Pending/Stale file tabs, citations, retrieval-only mode, and the
  Oracle LLM/CLI Agents rows.
- Added character-by-character Oracle answer streaming with an interval that
  is cleared before a new search and on component unmount.
- Added General, Projects, Devices, and Labs as the lighter design views.
- Kept all UI copy in English, used keyboard-reachable buttons for interactive
  controls, and kept feature styling on the existing global palette variables.
- Put replaceable fake values in `src/features/settings/mockData.ts` and
  `src/features/oracle/mockData.ts`. No npm dependency was added.

## Verification

All commands were run from `C:\Users\gualt\Desktop\New devboule\devboule-v2`.

### `pnpm exec tsc --noEmit`

```text
TSC_EXIT_CODE=0
```

### `pnpm build`

```text
> devboule-v2@0.1.0 build C:\Users\gualt\Desktop\New devboule\devboule-v2
> tsc --noEmit && vite build

vite v8.2.2 building client environment for production...
transforming...
✓ 31 modules transformed.
rendering chunks...
computing gzip size...
dist/index.html                                   0.44 kB │ gzip:  0.28 kB
dist/assets/Fraunces-Latin-ukD16Tqj.woff2        36.62 kB
dist/assets/JetBrainsMono-Latin-B9CIFXIH.woff2   40.40 kB
dist/assets/Inter-Latin-Dx4kXJAl.woff2           48.25 kB
dist/assets/index-CbLPAbFK.css                   45.52 kB │ gzip:  7.73 kB
dist/assets/index-CR_3tQdw.js                   189.14 kB │ gzip: 57.47 kB

✓ built in 214ms
BUILD_EXIT_CODE=0
```

### `pnpm tauri dev`

The final clean attempt compiled and launched the desktop application:

```text
Running BeforeDevCommand (`pnpm dev`)
VITE v8.2.2  ready in 352 ms
➜  Local:   http://localhost:1420/
Running DevCommand (`cargo  run --no-default-features --color always --`)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 9.61s
Running `C:\Users\gualt\Desktop\New devboule\devboule-v2\target\debug\devboule.exe`
```

The test window was then closed by terminating only its test PID. Because the
Tauri wrapper observes that forced teardown, its final process exit was `1`;
the application bootstrap itself completed and the `devboule.exe` window was
running. Earlier retries also exposed and cleaned only the test Vite/GUI
processes left by teardown (`Port 1420 is already in use` and a Windows file
lock on `target\debug\devboule.exe`).

## Dependencies

No dependency was added. The implementation uses the existing React 18.3.1,
TypeScript, Vite, and Zustand setup. The existing Tauri IPC boundary remains
the intended future integration point; M1b deliberately uses clearly marked
mock modules because the requested runtime commands are not implemented yet.

## Incomplete compared with the design/runtime

- Provider discovery, authentication proofs, model selection, Oracle doctor,
  indexing, watcher control, file pagination, citations, and answer streaming
  are UI mocks; they do not call the daemon yet.
- Lock app, Change indexed folder, default-model selectors, Add project, Pair a
  device, and CLI Agents are presentational controls until their IPC commands
  exist.
- The Oracle answer timer simulates the future stream and does not consume the
  existing Tauri `Channel` wrapper.
- The shared app composition file also contains the parallel Workspace agent's
  changes; those were left intact and were not included in the Settings/Oracle
  commit.
