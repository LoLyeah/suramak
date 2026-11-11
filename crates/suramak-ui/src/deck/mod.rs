//! The Deck — everything the desktop shell and the phone UI share.
//!
//! | module | what it is | desktop | phone |
//! |---|---|---|---|
//! | [`theme`] | the token ladder (colour, material, spacing, type) | yes | yes |
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
pub mod motion;
pub mod nav;
pub mod snapshot;
// M16I: the shared speaker badge + 8 px lane (desktop and phone draw the same).
pub mod speakers;
pub mod theme;
