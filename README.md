<!-- project:start -->
# DeskCrate

A lean, native Windows desktop organizer written in pure Rust. Grids on your desktop are live views of real folders. They can fold into smart capsules, or move into a drawer so the desktop stays empty.

**[Download v0.1.0](https://github.com/seiya058904/DeskCrate/releases/latest)** · [Source](https://github.com/seiya058904/DeskCrate)
<!-- project:end -->

<div align="center">

<h2>🗂️ A Tidy Desktop That Costs Almost Nothing</h2>

<p><strong>One 0.95 MB exe. About 6 MB of memory. 0 ms of CPU while idle.</strong></p>

<p>Grids that show real folders and fold into capsules on hover,<br>
with spring-driven motion you can interrupt at any point, the real Explorer menus, and drag and drop.</p>

<p>
  <a href="https://github.com/seiya058904/DeskCrate/releases/latest"><strong>⬇ Download</strong></a>
  &nbsp;·&nbsp;
  <a href="#-highlights">✨ Highlights</a>
  &nbsp;·&nbsp;
  <a href="#-footprint">📏 Footprint</a>
  &nbsp;·&nbsp;
  <a href="#-usage">🖱️ Usage</a>
  &nbsp;·&nbsp;
  <a href="#-how-it-works">🛠️ How It Works</a>
</p>

<p><sub>PURE RUST &nbsp;·&nbsp; WIN32 + DIRECT2D &nbsp;·&nbsp; MICA / ACRYLIC &nbsp;·&nbsp; 13 LANGUAGES &nbsp;·&nbsp; PORTABLE &nbsp;·&nbsp; GPL-3.0</sub></p>

</div>

---

> **No installer. No runtime. No background services.**
>
> Download one `.exe` and run it. A commented `config.toml` appears next to it, and your changes apply as soon as you save. The only registry write is the optional autostart entry.

> [!NOTE]
> DeskCrate is an unofficial, independent Rust re-implementation of [DeskBox](https://github.com/Tianyu199509/DeskBox) (GPL-3.0-only). It follows DeskBox's file-organizing experience and visual language. It is not affiliated with or endorsed by the DeskBox author, uses none of DeskBox's names, logos or icons, and never contacts DeskBox services.

## ✨ Highlights

<table>
  <tr>
    <td width="50%" valign="top">
      <h3>📁 Real Folders</h3>
      <p>Each grid is a view of an existing directory. Uninstalling DeskCrate or deleting its config never touches your files. Grids survive <code>Win+D</code> and re-attach after Explorer restarts.</p>
    </td>
    <td width="50%" valign="top">
      <h3>💊 Smart Capsules</h3>
      <p>A grid can stay open, expand on click, or expand on hover and fold when you leave. The capsule shows the title, the most recently changed item and the item count. Capsules docked low on the screen open upward.</p>
    </td>
  </tr>
  <tr>
    <td valign="top">
      <h3>🌊 Fluid, Interruptible Motion</h3>
      <p>Animation runs on a display-synced frame clock. Springs drive folding and the drawer, and they keep their velocity when you reverse midway. Hover highlights fade, presses sink and spring back, the wheel glides, and snapped edges glow.</p>
    </td>
    <td valign="top">
      <h3>🪟 Native Shell Integration</h3>
      <p>Explorer icons and thumbnails, the real context menu (including Send to, Open with and third-party entries) and OLE drag and drop. Move, copy, conflict dialogs and undo all behave as they do in Explorer.</p>
    </td>
  </tr>
  <tr>
    <td valign="top">
      <h3>🗄️ Drawer Mode</h3>
      <p>Keep the desktop empty. A global hotkey or a thin hot zone at a screen edge slides in one drawer, with a tab for each grid. The hot zone stays out of the way while full-screen apps run.</p>
    </td>
    <td valign="top">
      <h3>⌨️ Desktop-Style Details</h3>
      <p><sub>NAME EXPANSION · SLOW-CLICK RENAME · ARROW KEYS · CTRL+WHEEL ZOOM · PATH TOOLTIPS · AUTO-SCROLLING MARQUEE · SNAPPING</sub></p>
      <p>Small details from Windows Explorer and DeskBox, reproduced faithfully.</p>
    </td>
  </tr>
  <tr>
    <td valign="top">
      <h3>🔎 Type to Filter</h3>
      <p>Start typing in a focused grid or in the drawer to filter it. When <a href="https://www.voidtools.com/">Everything</a> is running, results from the whole disk appear below the local matches. Nothing extra is bundled.</p>
    </td>
    <td valign="top">
      <h3>📥 Minimal Auto Filing</h3>
      <p>Rules match on extension, keyword and idle days. Filing is off and dry-run by default. Files are only moved, never deleted or overwritten. Partial downloads are skipped, and every batch can be undone.</p>
    </td>
  </tr>
  <tr>
    <td valign="top">
      <h3>🌐 13 Languages</h3>
      <p><sub>ZH-CN · ZH-TW · EN-US · JA-JP · KO-KR · DE-DE · FR-FR · ES-ES · PT-BR · RU-RU · AR-SA · HI-IN · BN-BD</sub></p>
      <p>DeskCrate supports the same languages as DeskBox. It follows your Windows language, or set <code>[general] language</code> in the config.</p>
    </td>
    <td valign="top">
      <h3>🎒 Portable, No Assets</h3>
      <p>One <code>.exe</code> plus the <code>config.toml</code> next to it. The config reloads automatically and keeps your comments. The icons are drawn in code, so the repository contains no binary files.</p>
    </td>
  </tr>
</table>

## 📏 Footprint

<sub>Release build on Windows 11, measured with one grid of 53 items.</sub>

| Budget | Target | Measured |
| --- | --- | --- |
| Executable (static CRT, icon included) | < 5 MB | **0.95 MB** |
| Idle private memory | < 20 MB | **~6 MB** (working set ~1 MB) |
| Idle CPU | ~0 | **0.0 ms over 20 s** |
| Processes left after exit | none | **single process, exits cleanly** |

To reproduce these numbers, run `pwsh scripts/check-budget.ps1 -Build -Launch`.

## 🖱️ Usage

Launch `deskcrate.exe`. The first run writes a commented `config.toml` next to the exe, or to `%LOCALAPPDATA%\DeskCrate` if that folder is not writable. The default config contains a *Desktop* grid and a *Downloads* grid.

| Where | Action |
| --- | --- |
| **Grid title** | Drag the title to move the grid, or drag its edges to resize. Grids snap to screen edges and to each other, and the snapped edge glows. Double-click the title to rename the grid. |
| **Title buttons** | `⊕` adds a new grid. `…` opens the grid menu: rename, expand behaviour, lock position, lock size, open in Explorer, refresh, and delete the grid (its files are kept). `⌃` folds the grid. |
| **Items** | Select with a click, Ctrl, Shift or a marquee. Open with a double-click or Enter. Slow-click a name to rename it. Right-click for the Explorer menu. Drag items out or drop them in. Ctrl+wheel resizes the icons. |
| **Keyboard** | Arrows, `Home`/`End`, `PgUp`/`PgDn` · `F2` rename · `Del` delete to the Recycle Bin · `Ctrl+C/X/V/A` · `F5` refresh · `Alt+Enter` properties · `Apps` or `Shift+F10` opens the menu. Typing starts the filter; `Esc` clears it. |
| **Tray icon** | Left-click raises all grids until you click elsewhere; in drawer mode it toggles the drawer. Right-click opens the menu. |
| **`Ctrl+Alt+D`** | Toggles the drawer in drawer mode. In desktop mode it does the same as left-clicking the tray icon. You can change the shortcut. |

Everything else is set in `config.toml`: material, tint, corners, icon and text size, density, file extensions, thumbnails, language, capsule timings, drawer edge and width, and filing rules.

## 🛠️ How It Works

Every window is a plain Win32 window. DWM draws the Mica or Acrylic backdrop, the rounded corners and the border. Direct2D and DirectWrite render the content into one shared bitmap, and only when something changes. The renderer and its caches are released a few seconds after the last paint.

Nothing runs on a timer or polls while DeskCrate is idle:
- Folder changes arrive through `ReadDirectoryChangesW`.
- Hover uses `TrackMouseEvent`.
- Full-screen detection uses AppBar notifications.

[docs/AUDIT_AND_PLAN.md](docs/AUDIT_AND_PLAN.md) holds the source audit, the architecture decisions and an item-by-item comparison with DeskBox, written in Chinese.

### Build

Requirements:
- Windows 10 2004+ or Windows 11.
- The Rust toolchain pinned in `rust-toolchain.toml` (MSVC).
- Visual Studio Build Tools with the C++ workload.

```powershell
cargo build --release      # target\release\deskcrate.exe
cargo test                 # unit tests
```

## 📄 License

GPL-3.0-only, see [LICENSE](LICENSE). DeskCrate is a derivative of DeskBox © Tianyu Zhu, which is licensed under GPL-3.0-only. Everything is a product of voidtools; DeskCrate only talks to a running instance over its public IPC protocol.
