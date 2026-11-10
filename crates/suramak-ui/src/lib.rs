//! suramak-ui — the **Deck**: the shared, desktop-agnostic half of the app.
//!
//! The crate is one package with two targets:
//!
//! * **`lib`** (this file) is the Deck component library — the token ladder, the
//!   motion vocabulary, the painted control kit and the compact (phone) layout
//!   profile. It depends on `egui` and `suramak-core` only, so the Android shell
//!   (`crates/suramak-android`) compiles it for `aarch64-linux-android` and
//!   draws EXACTLY the desktop components.
//! * **`bin`** (`src/desktop.rs`, `src/main.rs`) is the desktop shell: winit,
//!   wgpu, `suramak-glass`, menus, single-instance, updater. It is gated to
//!   non-Android targets; none of it is reachable from the phone.
//!
//! The split is by module topology, not a second crate: the desktop shell still
//! needs the Deck components, and a `suramak-deck` crate would need the same
//! `egui`/`suramak-core` edges while adding a publish/version boundary that buys
//! nothing for one app. See `docs/reports/M13g1-compact-library.md` §1.

pub mod deck;
