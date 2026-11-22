# DeskCrate

A lean, native Windows desktop organizer written in pure Rust. Desktop "crates" (grids) are live views of **real folders**, can fold into smart capsules, or live in an on-demand drawer so the desktop stays completely empty.

> **Unofficial derivative.** DeskCrate is an independent Rust re-implementation that follows the file-organizing experience and visual language of [DeskBox](https://github.com/Tianyu199509/DeskBox) (GPL-3.0-only). It is not affiliated with or endorsed by the DeskBox author, uses none of DeskBox's names, logos or icons, and never contacts DeskBox services.

## Highlights

- **Real-folder grids**: every grid maps an existing directory. Uninstalling DeskCrate or deleting its config never touches your files.
- **Native shell integration**: Explorer icons, the real Explorer context menu (including third-party extensions, "Send to", "Open with"), and standard OLE drag and drop. Drops are handed to the folder's own shell drop target, so move/copy/link, conflict dialogs and undo behave exactly as in Explorer. Rename is inline and undoable.
- **Survives `Win+D`**: grids are owned by the desktop icon view, so Show Desktop leaves them in place. They re-attach automatically after Explorer restarts.
- **Smart capsules**: a grid can stay expanded, fold when you click it, or fold automatically (hover to expand, move away to collapse). Capsules docked in the lower half of a screen expand upward.
- **Drawer mode**: keeps the desktop empty. A global hotkey or a thin screen-edge hot zone slides in one drawer with a tab for each grid. The hot zone stays out of the way while full-screen apps run.
- **Type to filter**: start typing in a focused grid or the drawer. If [Everything](https://www.voidtools.com/) is running, matches from across the whole disk appear below the local results (over IPC; nothing is bundled).
- **Minimal auto filing**: rules match on extension, keyword and idle days, and folders are watched through events. Filing is off and dry-run by default. Files are only ever moved, never deleted or overwritten. Partial downloads are skipped, and every batch can be undone.
- **No assets**: the application icon and the tray glyph are drawn procedurally (the icon is rasterised by `build.rs` straight into a `.res`), so there is no rc.exe step and no binary files in the repository.
- **Portable**: one `.exe` plus `config.toml` beside it. The config hot-reloads and keeps your comments when DeskCrate writes positions back. The only registry write is the optional autostart entry.

## Footprint (measured, release build, Windows 11, two grids)

| Budget | Target | Measured |
| --- | --- | --- |
| Executable (static CRT, no runtime, icon included) | < 5 MB | **0.85 MB** |
| Idle private memory | < 20 MB | **~11 MB** (working set ~1 MB) |
| Idle CPU | ~0 | **0.0 ms CPU over 20 s** |
| Processes / leftovers on exit | none | single process, exits cleanly |

Run `pwsh scripts/check-budget.ps1 -Build -Launch` to reproduce these numbers.

## How it works

Every window is a plain Win32 window. DWM draws the Mica/Acrylic backdrop, rounded corners and border. Content is rendered by Direct2D (software) and DirectWrite into one shared DIB, and only when something changes. The renderer, its glyph caches and decoded bitmaps are released a few seconds after the last paint. There are no timers or polling while idle: directory changes arrive as `ReadDirectoryChangesW` completion routines on the UI thread, hover uses `TrackMouseEvent`, and full-screen detection uses AppBar notifications. See [docs/AUDIT_AND_PLAN.md](docs/AUDIT_AND_PLAN.md) for the source audit and the architecture decisions (written in Chinese).

## Build

Requirements: Windows 10 2004+ / Windows 11, the Rust toolchain pinned in `rust-toolchain.toml` (MSVC), and Visual Studio Build Tools (C++ workload).

```powershell
cargo build --release      # target\release\deskcrate.exe
cargo test                 # unit tests (layout, config, rules engine, IPC parsing…)
```

## Usage

Launch `deskcrate.exe`. On first start it writes a commented `config.toml` next to the executable, or to `%LOCALAPPDATA%\DeskCrate` if that folder is not writable. The default config contains a *Desktop* grid and a *Downloads* grid. Right-click the tray icon to add a grid, switch to drawer mode, run auto filing, edit the config, or exit.

| Where | Action |
| --- | --- |
| Grid title | Drag to move (snaps to screen edges and other grids, 5 DIP gap). Drag the edges to resize. Double-click to fold or unfold (click/smart modes). |
| Grid title `…` | Grid menu: open in Explorer, refresh, fold mode, lock, remove the grid (files are kept). |
| Items | Click / Ctrl / Shift / marquee to select. Double-click or Enter to open. Right-click for the Explorer menu. Drag out or drop in. |
| Keyboard (grid focused) | `F2` rename, `Del` delete to the Recycle Bin, `Ctrl+C/X/V`, `Ctrl+A`, `F5`, `Alt+Enter` properties. Typing starts the filter, `Esc` clears it. |
| `Ctrl+Alt+D` (configurable) | Drawer mode: toggle the drawer. Desktop mode: "peek" brings every grid to the front until you click elsewhere. |

All options (material, tint, corners, icon and text size, density, capsule timings, drawer edge and width, filing rules) live in `config.toml` and apply as soon as you save.

## License

GPL-3.0-only (see [LICENSE](LICENSE)). This project is a derivative of DeskBox © Tianyu Zhu, which is licensed under GPL-3.0-only. "Everything" is a product of voidtools; DeskCrate only talks to a running instance over its public IPC protocol.
