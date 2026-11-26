# DeskCrate

- Native Windows x64 Rust app. `src/app.rs` owns the Win32 event loop and window reconciliation, `src/ui/` the Direct2D/DirectWrite views, `src/win/` OS integration and `src/shell/` Explorer behavior. See `docs/AUDIT_AND_PLAN.md` for architectural decisions.
- Keep idle work event-driven and the MSVC runtime statically linked. Grids are views of real folders. Removing a grid only changes configuration. Delegate drops to the folder's Shell target and preserve the non-overwriting, undoable filing rules in `src/sort.rs`.
- Keep `src/config.rs` edits comment-preserving and compatible with hot reload. Update all supported translations in `src/i18n.rs` when changing visible strings.
- Use the pinned `rust-toolchain.toml` and Visual Studio C++ Build Tools. Check with `cargo fmt --check` and `cargo test`. Verify shell and window changes on Windows using disposable folders. `pwsh scripts/check-budget.ps1 -Build -Launch` checks size, idle memory/CPU and clean exit.
- `cargo build --release` produces `target/release/deskcrate.exe`. GitHub Releases distribute the portable EXE and matching `SHA256SUMS.txt`. Preserve GPL notices and source availability for binary releases.
