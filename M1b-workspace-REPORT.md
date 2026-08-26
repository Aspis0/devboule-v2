# M1b — Workspace

## Built

- Replaced the Workspace placeholder with the three-column surface from the design: project/workspace list, agent or terminal session, and a multi-surface side panel.
- Added the left and right panel splitters with the design values: 252 px / 366 px initial widths, 180–460 px drag limits, 30 px collapsed state, double-click collapse, and keyboard resizing.
- Added the mock session tabs, terminal output, user/tool/agent conversation messages, streaming caret, local composer behavior (`Enter` sends, `Shift+Enter` inserts a line break), turn hint, and token-rate label.
- Added the inline permission card with the three requested status labels: `Waiting on you`, `Allowed once · running`, and `Denied — the turn continues without it`.
- Added the Changes diff with `Stage` / `Discard`, Files tree, Interactive app preview with mock reload/build counter, Design generation surface, Pull request summary, and the Worktree → Preview → Review → Commit → PR → Merge ship strip.
- Added keyboard-accessible controls for actions that are mouse-accessible, including panel collapse, splitters, lists, tabs, surface menu, permission actions, diff actions, and composer controls.

## Mock boundary

`src/features/workspace/mockData.ts` contains all fixture data and is explicitly marked as mock-only. No Tauri command is invoked. Replacing the fixture module with IPC adapters is intentionally mechanical.

## Verification

Commands were run from the repository root.

### TypeScript

`pnpm exec tsc --noEmit`

Result: exit code `0`.

### Production build

`pnpm build`

Result: exit code `0`.

Relevant output:

```text
vite v8.2.2 building client environment for production...
✓ 31 modules transformed.
✓ built in 217ms
```

### Tauri dev run

`pnpm tauri dev`

Result: Rust dev profile completed and `target\debug\devboule.exe` launched successfully.

Relevant output:

```text
VITE v8.2.2  ready in 360 ms
➜  Local:   http://localhost:1420/
Finished `dev` profile [unoptimized + debuginfo]
Running `...\target\debug\devboule.exe`
```

The in-app browser runtime was unavailable in this environment, so no automated screenshot/DOM pass was possible. The Tauri process itself did launch successfully.

## Dependencies

No dependencies were added. The implementation uses the existing React, TypeScript, Vite, and Zustand setup; Workspace interaction state is local React state so composer keystrokes do not update the global app store.

## Remaining mock/incomplete behavior

- Projects, sessions, terminal output, permissions, diffs, tests, app preview, design generation, and PR state are local fixtures only; there is no backend or filesystem operation yet.
- `Open Design`, the model selector, file rows, preview actions, and generation controls are present as accessible mock controls but do not call another surface or service.
- The exact design uses two extra off-palette diff background shades. To keep the feature on the existing CSS palette, added/removed diff rows use `color-mix()` from `--green` / `--danger` rather than introducing new hex colors.
- The permission card remains inline after `Allow once` / `Deny` with disabled actions so each requested status remains visible; this makes the three states inspectable while preserving the design’s inline, non-modal treatment.
