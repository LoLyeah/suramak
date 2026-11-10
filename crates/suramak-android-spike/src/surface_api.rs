//! The surface seam between the M13b window machinery and the G3 product
//! shell.
//!
//! `app.rs` still owns the winit/wgpu/egui generation and the lifecycle gates;
//! it no longer knows which UI it draws. The APK's library (`suramak-android`)
//! supplies a [`CompactSurface`]: it folds the Kotlin IME bridge into egui's
//! input, draws `suramak_ui::deck::compact` from the one `CompactApp` the JNI
//! host owns, and dispatches the resulting actions.
//!
//! This keeps the diagnostic state (`SpikeState`, `diag`) and the four-check
//! counters in the spike crate while the product's host state lives in the
//! crate that also exports the JNI bridge — one copy of each, loaded from one
//! `.so`.

use egui::{Context, RawInput};

use suramak_ui::deck::compact::CompactAction;

use crate::ui::SpikeState;

/// One product surface owned by the frame loop.
pub trait CompactSurface {
    /// Folds the soft keyboard's buffer into this frame's input events.
    fn prime_input(&mut self, raw: &mut RawInput, state: &SpikeState);

    /// Draws one frame and returns the host actions it requested.
    fn draw(&mut self, ctx: &Context, state: &SpikeState) -> Vec<CompactAction>;

    /// Runs the previous frame's actions, after the app borrow has ended.
    fn dispatch(&mut self, actions: Vec<CompactAction>);
}
