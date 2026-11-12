//! The Deck — everything the desktop shell and the phone UI share.
//!
//! | module | what it is | desktop | phone |
//! |---|---|---|---|
//! | [`theme`] | the token ladder (colour, material, spacing, type) | yes | yes |
//! | [`material`] | the frosted primitives + the SystemGlass/PaintedFrost capability | yes | yes |
//! | [`icons`] | the painted 1.75 px icon kit (18/20 pt) | yes | yes |
//! | [`motion`] | duration tokens, easing, springs, Reduce Motion | yes | yes |
//! | [`controls`] | painted control kit (buttons, rows, toggles, sheets) | new compact screens | yes |
//! | [`nav`] | push/pop stack, back semantics, transition clock | compact routes | yes |
//! | [`compact`] | the phone layout profile (Library / editor / Settings) | tests only | yes |
//! | [`snapshot`] | deterministic host rasteriser for the phone-viewport suite | tests only | tests only |
//!
//! Desktop-only code (the window shell, menus, export dialogs, HUD panel
//! surgery) lives in the binary target, not here.

pub mod bounds;
pub mod compact;
pub mod controls;
// M20A: the bundled OFL faces (embedded statics) + their platform table.
pub mod fonts;
// M20B: the painted icon kit (1.75 px round-cap strokes at 18/20 pt).
pub mod icons;
// M20B: the material capability + the frosted primitives (veil, rim, grain).
pub mod material;
pub mod motion;
pub mod nav;
// M22B: the quote card the voice-note notification shares (pure raster).
pub mod quote_card;
pub mod snapshot;
// M16I: the shared speaker badge + 8 px lane (desktop and phone draw the same).
pub mod speakers;
pub mod theme;
// M20A: the plan's type scale as tokens (FontId + tracking + line height).
pub mod type_scale;
