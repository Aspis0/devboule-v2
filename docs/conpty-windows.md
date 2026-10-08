# ConPTY on Windows: the app-local bundle

## What

The vendored `portable-pty` loader (`vendor/portable-pty/src/win/conpty_loader.rs`)
picks a ConPTY implementation once, at daemon startup (the daemon reports it
before serving; otherwise the first pseudoconsole would). It uses an app-local
bundle only when `conpty\conpty.dll` **and** `conpty\x64\OpenConsole.exe` sit in
the running executable's own directory as plain files — a symlink or junction
is rejected when the bundle is checked, immediately before the load and not
atomically with it (a hard link is not a reparse point) — and the pair is
trusted together or not at all. The DLL is
loaded through `LoadLibraryExW` pinned to its own directory plus System32;
anything short of a loadable, symbol-complete table falls back to kernel32's
inbox implementation, which every supported Windows still ships. On any
architecture other than x64 the bundle is not consulted (the pinned shim is
x64-only). The daemon prints the one-time choice to its log (`daemon.log`) at
startup, one of:

- `ConPTY: using the app-local ConPTY bundle beside the executable`
- `ConPTY: using the Windows inbox ConPTY (kernel32)`
- `ConPTY: using the Windows inbox ConPTY (a staged ConPTY bundle was present but could not be loaded: …)`
  — the downgrade keeps its reason, never a user path.

A `conpty.dll` in the current directory or on `PATH` is never considered.

## Why

Two reasons to prefer an app-local `conpty.dll` over the inbox one:

- **Search-order safety.** Opening `conpty.dll` by bare name — what
  `portable-pty 0.9` did upstream — lets any look-alike file in a `PATH`
  folder or the current directory load into the daemon, the process that owns
  every pseudoconsole. The loader now resolves one absolute path beside the
  executable and nothing else.
- **Windows 10.** The inbox ConPTY there froze at the 19041-era host; its
  resize and OSC defects are documented upstream (microsoft/terminal #8880,
  #15551). A bundled 1.24 host fixes those once the bundle ships.

## How to run

`scripts/stage-conpty.ps1` stages the pinned Microsoft package.
`pnpm build:installer` runs it as part of the installer build (staging into
`target\release`); ordinary dev builds and the gate never run it, and the
script's only network access is the one pinned download:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\stage-conpty.ps1
# or, beside a release build, or from a local copy of the .nupkg:
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\stage-conpty.ps1 -TargetDir target\release
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\stage-conpty.ps1 -PackagePath C:\temp\Microsoft.Windows.Console.ConPTY.1.24.260710001.nupkg
```

It verifies the package SHA-256 against `packaging/windows/conpty.json`,
extracts only the pinned members, checks each file's SHA-256, PE machine type
and Microsoft Authenticode signature, and stages everything into a temporary
directory inside the target first. The target's previous `conpty\` and
`THIRD-PARTY-NOTICES\` are deleted before the download begins, so any failure
after that point — a failed download, a hash or signature mismatch, an
interrupted run — leaves no stale bundle for a later installer build to ship:
the loader stays on the inbox, and the next bundle step fails loudly on the
missing resource rather than packaging old files. The success line prints
only after the verified tree is fully in place. The script refuses a target
or a destination that is a reparse point — checked before the deletion, which
must not follow a link — and refuses a drive root, and a staged
`target\debug` survives `cargo clean`. The pin values are taken from herdr's
packaging (see NOTICE); the licence and notice texts shipped with the
binaries come from `packaging/windows/licenses/`.

## Where the files must sit

The staged tree sits **beside `devboule-daemon.exe`**: `conpty\conpty.dll`,
`conpty\x64\OpenConsole.exe`, `conpty\arm64\OpenConsole.exe`, and
`THIRD-PARTY-NOTICES\Microsoft.Windows.Console.ConPTY-LICENSE.txt` plus
`…-NOTICE.md` — the layout herdr's installer deploys and verifies
(`herdr-src/distribution/install.ps1`). The daemon executable's directory is
the loader's only lookup, and the daemon, not the GUI, owns the
pseudoconsoles. The NSIS installer (`src-tauri/tauri.installer.conf.json`) ships this tree
with the daemon: `pnpm build:installer` stages the pinned pair and both notice
files into `target\release`, and the installer's `bundle.resources` places
`conpty\` and `THIRD-PARTY-NOTICES\` in the install root beside
`devboule-daemon.exe`, where the loader looks for them. An installed app on
x64 therefore starts on the pinned app-local host (its `daemon.log` says so).
A run from an ordinary dev build still gets the inbox ConPTY unless the tree
is staged beside `target\debug\devboule-daemon.exe` by hand — the manual
recipe above.
