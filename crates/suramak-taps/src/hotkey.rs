//! Global hotkeys (M16B) — **Carbon `RegisterEventHotKey`**, hand-written FFI.
//!
//! ## Why Carbon (and what it costs)
//!
//! `RegisterEventHotKey` is the one macOS global-hotkey API that needs **no TCC
//! permission**: the chord is intercepted by the window server / Carbon event
//! dispatcher before the focused app sees it (so the other app never receives
//! the keystroke either). It requires the process's **main thread with a running
//! event loop** — the winit loop is exactly that, and the registration entry
//! point refuses to run anywhere else.
//!
//! Media-key chords (play/pause, volume) are **not** Carbon hotkeys: they need a
//! `CGEventTap` plus the Input Monitoring permission and are out of scope here
//! (regular keys only). See `docs/reports/M16b-hotkey-hud-macos.md`.
//!
//! ## Marshalling
//!
//! Carbon calls [`hotkey_event_handler`] on the main thread inside the Carbon
//! event dispatch. The handler never touches egui or AppKit state: it invokes
//! the closure the caller registered, and the app's closure posts the existing
//! M12 `EventLoopProxy` wake (`EventLoopProxy::send_event`, asynchronous and
//! non-blocking). A synchronous dispatch here would deadlock against the egui
//! context lock — the same rule the M12 repaint wake documents.
//!
//! ## Shape
//!
//! [`HotkeyChord`] is the platform-neutral chord value (parse/display here,
//! serde in the UI's `HudSettings`). [`HotkeyRegistration`] is the state
//! machine: it owns exactly zero or one live registration, `change()` unregisters
//! the old chord **before** registering the new one and restores the old chord
//! when the new one is refused, and `Drop` unregisters. The state machine is
//! generic over [`HotkeyBackend`] so the host tests exercise every transition
//! with a fake backend; the real backend is Carbon on macOS and an
//! `Unsupported` stub everywhere else.

use std::fmt;
use std::sync::Arc;

/// A key a global hotkey can use. Letters and Space cover the product's chord
/// set; function/media keys are deliberately absent (media keys need a
/// CGEventTap + Input Monitoring — out of scope for M16.2/M16.3-macOS).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HotkeyKey {
    /// An ASCII letter, normalised to uppercase.
    Letter(char),
    /// The space bar.
    Space,
}

impl HotkeyKey {
    /// The key's display label (`R`, `Space`).
    pub fn label(self) -> String {
        match self {
            HotkeyKey::Letter(letter) => letter.to_string(),
            HotkeyKey::Space => "Space".to_string(),
        }
    }

    /// The Carbon virtual key code (`kVK_ANSI_*` / `kVK_Space`), when the key is
    /// one this crate accepts.
    #[cfg(target_os = "macos")]
    fn carbon_code(self) -> Option<u32> {
        Some(match self {
            // HIToolbox/Events.h `kVK_ANSI_*`. The ANSI map is not alphabetical:
            // A=0x00, S=0x01, D=0x02, F=0x03, H=0x04, G=0x05, Z=0x06, X=0x07,
            // C=0x08, V=0x09, B=0x0B, Q=0x0C, W=0x0D, E=0x0E, R=0x0F, Y=0x10,
            // T=0x11, O=0x1F, U=0x20, I=0x22, P=0x23, L=0x25, J=0x26, K=0x28,
            // N=0x2D, M=0x2E.
            HotkeyKey::Letter('A') => 0x00,
            HotkeyKey::Letter('S') => 0x01,
            HotkeyKey::Letter('D') => 0x02,
            HotkeyKey::Letter('F') => 0x03,
            HotkeyKey::Letter('H') => 0x04,
            HotkeyKey::Letter('G') => 0x05,
            HotkeyKey::Letter('Z') => 0x06,
            HotkeyKey::Letter('X') => 0x07,
            HotkeyKey::Letter('C') => 0x08,
            HotkeyKey::Letter('V') => 0x09,
            HotkeyKey::Letter('B') => 0x0B,
            HotkeyKey::Letter('Q') => 0x0C,
            HotkeyKey::Letter('W') => 0x0D,
            HotkeyKey::Letter('E') => 0x0E,
            HotkeyKey::Letter('R') => 0x0F,
            HotkeyKey::Letter('Y') => 0x10,
            HotkeyKey::Letter('T') => 0x11,
            HotkeyKey::Letter('O') => 0x1F,
            HotkeyKey::Letter('U') => 0x20,
            HotkeyKey::Letter('I') => 0x22,
            HotkeyKey::Letter('P') => 0x23,
            HotkeyKey::Letter('L') => 0x25,
            HotkeyKey::Letter('J') => 0x26,
            HotkeyKey::Letter('K') => 0x28,
            HotkeyKey::Letter('N') => 0x2D,
            HotkeyKey::Letter('M') => 0x2E,
            HotkeyKey::Space => 0x31, // kVK_Space
            HotkeyKey::Letter(_) => return None,
        })
    }
}

/// One global-hotkey chord. At least one of Command, Option or Control is
/// required: a Shift-only global chord would fire on ordinary typing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HotkeyChord {
    pub key: HotkeyKey,
    pub command: bool,
    pub shift: bool,
    pub option: bool,
    pub control: bool,
}

/// The shipped default: **⌘⇧Space**. Chosen over ⌘⇧R because that chord is
/// Safari's Reader and Chrome's hard reload — a global hotkey would steal it
/// from both (the reason is in [`advisory_collision`] and the Settings caption).
pub const DEFAULT_HOTKEY: HotkeyChord = HotkeyChord {
    key: HotkeyKey::Space,
    command: true,
    shift: true,
    option: false,
    control: false,
};

impl Default for HotkeyChord {
    fn default() -> Self {
        DEFAULT_HOTKEY
    }
}

/// Why a chord could not be parsed. Messages name the accepted grammar so the
/// Settings field can show them verbatim.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChordError {
    #[error("a hotkey needs a modifier and a key (for example ⌘⇧Space)")]
    Empty,
    #[error("unknown modifier {0:?} — use cmd, shift, opt or ctrl")]
    UnknownModifier(String),
    #[error("a hotkey needs a key (for example ⌘⇧Space)")]
    MissingKey,
    #[error("a hotkey can only have one key (got {0:?} and {1:?})")]
    MultipleKeys(String, String),
    #[error("unsupported key {0:?} — use a letter A–Z or Space")]
    UnsupportedKey(String),
    #[error("a global hotkey needs ⌘, ⌥ or ⌃ — Shift alone would fire while typing")]
    ShiftOnly,
    #[error("a hotkey needs at least one of ⌘, ⌥ or ⌃")]
    NoModifier,
}

impl HotkeyChord {
    /// Parses the Settings grammar: modifier tokens (`cmd`/`command`/`⌘`,
    /// `shift`/`⇧`, `opt`/`option`/`alt`/`⌥`, `ctrl`/`control`/`⌃`) plus one
    /// letter or `space`, separated by `+` or whitespace. Modifier glyphs may
    /// also be written compactly (`⌘⇧R`). Case-insensitive.
    pub fn parse(text: &str) -> Result<Self, ChordError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(ChordError::Empty);
        }

        // Split on '+' and whitespace; compact glyph runs ("⌘⇧R") are peeled
        // one glyph at a time because they contain no separator.
        let mut tokens: Vec<String> = Vec::new();
        for chunk in trimmed
            .split(['+', ' '])
            .filter(|part| !part.trim().is_empty())
        {
            let mut rest = chunk.trim();
            while let Some(glyph) = rest.chars().next() {
                if matches!(glyph, '⌘' | '⇧' | '⌥' | '⌃') {
                    tokens.push(glyph.to_string());
                    rest = &rest[glyph.len_utf8()..];
                } else {
                    break;
                }
            }
            if !rest.is_empty() {
                tokens.push(rest.to_string());
            }
        }

        let mut chord = HotkeyChord {
            key: HotkeyKey::Space, // placeholder; `key` is set exactly once below
            command: false,
            shift: false,
            option: false,
            control: false,
        };
        let mut key: Option<(HotkeyKey, String)> = None;

        for token in tokens {
            match token.to_ascii_lowercase().as_str() {
                "cmd" | "command" | "⌘" => chord.command = true,
                "shift" | "⇧" => chord.shift = true,
                "opt" | "option" | "alt" | "⌥" => chord.option = true,
                "ctrl" | "control" | "⌃" => chord.control = true,
                "space" | "spacebar" => {
                    set_key(&mut key, HotkeyKey::Space, &token)?;
                }
                other => {
                    let mut chars = other.chars();
                    match (chars.next(), chars.next()) {
                        (Some(letter), None) if letter.is_ascii_alphabetic() => {
                            set_key(
                                &mut key,
                                HotkeyKey::Letter(letter.to_ascii_uppercase()),
                                &token,
                            )?;
                        }
                        _ => {
                            // A glyph token we already peeled would not reach
                            // here; anything else is either an unknown modifier
                            // or an unsupported key.
                            if looks_like_modifier(other) {
                                return Err(ChordError::UnknownModifier(token));
                            }
                            return Err(ChordError::UnsupportedKey(token));
                        }
                    }
                }
            }
        }

        let Some((key, _)) = key else {
            return Err(ChordError::MissingKey);
        };
        chord.key = key;
        chord.validate()?;
        Ok(chord)
    }

    /// The chord's grammar rules beyond parsing (also applied by `parse`).
    pub fn validate(&self) -> Result<(), ChordError> {
        if let HotkeyKey::Letter(letter) = self.key {
            if !letter.is_ascii_alphabetic() || letter.is_ascii_lowercase() {
                return Err(ChordError::UnsupportedKey(letter.to_string()));
            }
        }
        if !(self.command || self.option || self.control) {
            return if self.shift {
                Err(ChordError::ShiftOnly)
            } else {
                Err(ChordError::NoModifier)
            };
        }
        Ok(())
    }

    /// The modifier run as it is displayed (`⌘⇧`).
    pub fn modifiers_label(&self) -> String {
        let mut label = String::new();
        if self.control {
            label.push('⌃');
        }
        if self.option {
            label.push('⌥');
        }
        if self.shift {
            label.push('⇧');
        }
        if self.command {
            label.push('⌘');
        }
        label
    }

    /// The full displayed chord (`⌘⇧Space`).
    pub fn label(&self) -> String {
        format!("{}{}", self.modifiers_label(), self.key.label())
    }

    /// The canonical Settings spelling (`cmd+shift+space`, `cmd+shift+r`).
    pub fn canonical(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.command {
            parts.push("cmd".to_string());
        }
        if self.shift {
            parts.push("shift".to_string());
        }
        if self.option {
            parts.push("opt".to_string());
        }
        if self.control {
            parts.push("ctrl".to_string());
        }
        match self.key {
            HotkeyKey::Letter(letter) => parts.push(letter.to_ascii_lowercase().to_string()),
            HotkeyKey::Space => parts.push("space".to_string()),
        }
        parts.join("+")
    }
}

impl fmt::Display for HotkeyChord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label())
    }
}

fn looks_like_modifier(token: &str) -> bool {
    matches!(
        token,
        "fn" | "function" | "hyper" | "meh" | "super" | "win" | "meta"
    )
}

fn set_key(
    slot: &mut Option<(HotkeyKey, String)>,
    key: HotkeyKey,
    original: &str,
) -> Result<(), ChordError> {
    if let Some((_, previous)) = slot {
        return Err(ChordError::MultipleKeys(
            previous.clone(),
            original.to_string(),
        ));
    }
    *slot = Some((key, original.to_string()));
    Ok(())
}

/// A chord the **system** owns. Registration refuses these before touching
/// Carbon, with the owner named so Settings can say why.
pub fn reserved_by_system(chord: &HotkeyChord) -> Option<&'static str> {
    use HotkeyKey::{Letter, Space};
    match (
        chord.key,
        chord.command,
        chord.shift,
        chord.option,
        chord.control,
    ) {
        // Spotlight.
        (Space, true, false, false, false) => Some("Spotlight search"),
        // Emoji & Symbols / Character Viewer.
        (Space, true, false, false, true) => Some("Emoji & Symbols"),
        // Log Out (the confirming variant; ⌘⌥⇧Q logs out immediately and is
        // not expressible with this key set because it needs Q — it is).
        (Letter('Q'), true, true, false, false) => Some("Log Out"),
        // Force Quit is ⌘⌥Esc — Escape is not an accepted key here.
        _ => None,
    }
}

/// A chord **Suramak's own menu** owns (winit's standard macOS app menu). A
/// global hotkey on one of these would intercept the menu shortcut everywhere.
pub fn reserved_by_app(chord: &HotkeyChord) -> Option<&'static str> {
    use HotkeyKey::Letter;
    match (
        chord.key,
        chord.command,
        chord.shift,
        chord.option,
        chord.control,
    ) {
        (Letter('Q'), true, false, false, false) => Some("Suramak's Quit (⌘Q)"),
        (Letter('W'), true, false, false, false) => Some("Suramak's Close Window (⌘W)"),
        (Letter('H'), true, false, false, false) => Some("Suramak's Hide (⌘H)"),
        (Letter('M'), true, false, false, false) => Some("Suramak's Minimize (⌘M)"),
        _ => None,
    }
}

/// Common **application** shortcuts that a global chord would shadow. Not a
/// registration refusal (another app may not even be installed): the Settings
/// section shows this as an advisory next to the chord field.
pub fn advisory_collision(chord: &HotkeyChord) -> Option<&'static str> {
    use HotkeyKey::Letter;
    match (
        chord.key,
        chord.command,
        chord.shift,
        chord.option,
        chord.control,
    ) {
        (Letter('R'), true, true, false, false) => {
            Some("Safari's Reader and Chrome's hard reload (⌘⇧R)")
        }
        (Letter('S'), true, true, false, false) => Some("Save As (⌘⇧S) in most apps"),
        (Letter('V'), true, true, false, false) => Some("Paste and Match Style (⌘⇧V)"),
        (Letter('T'), true, true, false, false) => Some("Reopen Tab (⌘⇧T) in browsers"),
        _ => None,
    }
}

/// Errors from chord validation and hotkey registration.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HotkeyError {
    /// Global hotkeys are Carbon/macOS-only; this build cannot register one.
    #[error("global hotkeys are not supported on this platform (Carbon RegisterEventHotKey is macOS-only)")]
    Unsupported,
    /// Carbon must be called on the OS main thread with a running event loop.
    #[error("global hotkeys must be registered on the OS main thread")]
    NotMainThread,
    /// The chord is parsed but invalid for a global hotkey.
    #[error("invalid hotkey: {0}")]
    Invalid(#[from] ChordError),
    /// The system owns this chord (see [`reserved_by_system`]).
    #[error("{chord} is reserved by the system ({owner})")]
    Reserved {
        chord: HotkeyChord,
        owner: &'static str,
    },
    /// Suramak's own menu owns this chord (see [`reserved_by_app`]).
    #[error("{chord} is used by {owner}")]
    AppReserved {
        chord: HotkeyChord,
        owner: &'static str,
    },
    /// Carbon reported `eventHotKeyExistsErr`: another registration (this app
    /// or another) already owns the chord.
    #[error("{chord} is already registered — choose a different chord")]
    Conflict { chord: HotkeyChord },
    /// A Carbon call returned a non-zero `OSStatus`.
    #[error("Carbon call {call} failed (OSStatus {status})")]
    Carbon { call: &'static str, status: i32 },
}

/// The closure a hotkey fires. It must only post to the event loop (the M12
/// `EventLoopProxy` wake) — never block, never touch egui.
pub type HotkeyHandler = Arc<dyn Fn() + Send + Sync + 'static>;

/// One backend registration handle. Opaque to callers; the fake backend in the
/// tests uses a counter.
#[doc(hidden)]
pub trait HotkeyBackend {
    type Handle: Copy + PartialEq + fmt::Debug;

    fn register(
        &mut self,
        chord: HotkeyChord,
        handler: HotkeyHandler,
    ) -> Result<Self::Handle, HotkeyError>;

    fn unregister(&mut self, handle: Self::Handle);
}

/// A live global hotkey. At most one chord is registered at a time; changing
/// the chord unregisters the old one first (Carbon has no atomic replace) and
/// restores it if the new chord is refused. Dropping the value unregisters.
pub struct HotkeyRegistration<B: HotkeyBackend = platform::Platform> {
    backend: B,
    handler: HotkeyHandler,
    current: Option<(HotkeyChord, B::Handle)>,
}

impl<B: HotkeyBackend> HotkeyRegistration<B> {
    fn with_backend(backend: B, handler: HotkeyHandler) -> Self {
        Self {
            backend,
            handler,
            current: None,
        }
    }

    /// The chord currently registered, if any.
    pub fn chord(&self) -> Option<HotkeyChord> {
        self.current.as_ref().map(|(chord, _)| *chord)
    }

    /// Registers `chord`, replacing the current one. The old registration is
    /// removed **before** the new one is attempted; if the new chord is
    /// refused (system-reserved, already registered, invalid), the old chord is
    /// re-registered so a failed Settings edit never disables dictation.
    pub fn change(&mut self, chord: HotkeyChord) -> Result<(), HotkeyError> {
        chord.validate()?;
        if let Some(owner) = reserved_by_system(&chord) {
            return Err(HotkeyError::Reserved { chord, owner });
        }
        if let Some(owner) = reserved_by_app(&chord) {
            return Err(HotkeyError::AppReserved { chord, owner });
        }
        if self.chord() == Some(chord) {
            return Ok(());
        }
        let previous = self.current.take();
        if let Some((_, handle)) = previous {
            self.backend.unregister(handle);
        }
        match self.backend.register(chord, Arc::clone(&self.handler)) {
            Ok(handle) => {
                self.current = Some((chord, handle));
                Ok(())
            }
            Err(err) => {
                // Best-effort restore: a backend that cannot re-register the
                // old chord leaves dictation unbound and the error says why.
                if let Some((old_chord, _)) = previous {
                    if let Ok(handle) = self.backend.register(old_chord, Arc::clone(&self.handler))
                    {
                        self.current = Some((old_chord, handle));
                    }
                }
                Err(err)
            }
        }
    }

    /// Unregisters. Idempotent (an empty slot is a no-op) and also run by
    /// `Drop`, so every teardown path — HUD disabled, chord changed, app quit —
    /// releases the Carbon registration exactly once.
    pub fn unregister(mut self) {
        self.clear();
    }

    fn clear(&mut self) {
        if let Some((_, handle)) = self.current.take() {
            self.backend.unregister(handle);
        }
    }
}

impl<B: HotkeyBackend> Drop for HotkeyRegistration<B> {
    fn drop(&mut self) {
        self.clear();
    }
}

/// Registers `chord` with the platform backend. On macOS this is Carbon; on
/// every other platform it returns [`HotkeyError::Unsupported`] without side
/// effects. The handler is called on the OS main thread.
pub fn register_hotkey(
    chord: HotkeyChord,
    handler: HotkeyHandler,
) -> Result<HotkeyRegistration, HotkeyError> {
    let mut registration = HotkeyRegistration::with_backend(platform::Platform::new(), handler);
    registration.change(chord)?;
    Ok(registration)
}

#[cfg(target_os = "macos")]
mod platform {
    //! Hand-written Carbon bindings. No crate: `Carbon.framework` has shipped
    //! `RegisterEventHotKey` since Mac OS X 10.0 and the ABI below is the
    //! documented `CarbonEvents.h` shape.
    use std::ffi::c_void;
    use std::ptr;
    use std::sync::{Mutex, OnceLock};

    use super::{HotkeyChord, HotkeyError, HotkeyHandler};

    type OsStatus = i32;
    type EventRef = *mut c_void;
    type EventTargetRef = *mut c_void;
    type EventHandlerRef = *mut c_void;
    type EventHandlerCallRef = *mut c_void;
    type EventHotKeyRef = *mut c_void;

    /// `EventHotKeyID { OSType signature; UInt32 id; }`.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EventHotKeyID {
        signature: u32,
        id: u32,
    }

    /// `EventTypeSpec { UInt32 eventClass; UInt32 eventKind; }`.
    #[repr(C)]
    struct EventTypeSpec {
        event_class: u32,
        event_kind: u32,
    }

    type EventHandlerProcPtr =
        Option<unsafe extern "C" fn(EventHandlerCallRef, EventRef, *mut c_void) -> OsStatus>;

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> EventTargetRef;
        fn RegisterEventHotKey(
            in_hot_key_code: u32,
            in_hot_key_modifiers: u32,
            in_hot_key_id: EventHotKeyID,
            in_target: EventTargetRef,
            in_options: u32,
            out_ref: *mut EventHotKeyRef,
        ) -> OsStatus;
        fn UnregisterEventHotKey(in_hot_key: EventHotKeyRef) -> OsStatus;
        fn InstallEventHandler(
            in_target: EventTargetRef,
            in_handler: EventHandlerProcPtr,
            in_num_types: usize,
            in_list: *const EventTypeSpec,
            in_user_data: *mut c_void,
            out_ref: *mut EventHandlerRef,
        ) -> OsStatus;
        fn RemoveEventHandler(in_handler: EventHandlerRef) -> OsStatus;
    }

    // CarbonEvents.h `kEventClassKeyboard` / `kEventHotKeyPressed`.
    const K_EVENT_CLASS_KEYBOARD: u32 = fourcc(*b"keyb");
    const K_EVENT_HOT_KEY_PRESSED: u32 = 5;
    // Carbon modifier masks (Events.h): cmdKey, shiftKey, optionKey, controlKey.
    const CMD_KEY: u32 = 0x0100;
    const SHIFT_KEY: u32 = 0x0200;
    const OPTION_KEY: u32 = 0x0800;
    const CONTROL_KEY: u32 = 0x1000;
    /// `eventHotKeyExistsErr` (CarbonEvents.h) — the chord is already owned.
    const EVENT_HOT_KEY_EXISTS_ERR: i32 = -9878;
    /// Four-char signature for this app's one hotkey (`'SurK'`).
    const SIGNATURE: u32 = fourcc(*b"SurK");

    const fn fourcc(bytes: [u8; 4]) -> u32 {
        u32::from_be_bytes(bytes)
    }

    /// The single Carbon callback slot. A `Mutex` (not a lock-free cell) is
    /// deliberate: the callback runs on the main thread, registration and
    /// teardown run on the main thread too, so contention is nil and the
    /// `Mutex` keeps the unsafety in one place.
    static HANDLER: Mutex<Option<HotkeyHandler>> = Mutex::new(None);
    /// Serialises registration/teardown if a caller ever gets clever; the
    /// manager itself already holds one slot.
    static REGISTRATION: OnceLock<Mutex<()>> = OnceLock::new();

    fn registration_lock() -> &'static Mutex<()> {
        REGISTRATION.get_or_init(|| Mutex::new(()))
    }

    unsafe extern "C" fn hotkey_event_handler(
        _call: EventHandlerCallRef,
        _event: EventRef,
        _user_data: *mut c_void,
    ) -> OsStatus {
        let handler = HANDLER.lock().ok().and_then(|slot| slot.clone());
        if let Some(handler) = handler {
            // The handler is ours (a non-blocking EventLoopProxy send). A panic
            // must never unwind through Carbon's C frames.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| handler()));
        }
        0
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Handle {
        hotkey: usize,
        event_handler: usize,
    }

    pub struct Platform {
        _private: (),
    }

    impl Platform {
        pub fn new() -> Self {
            Self { _private: () }
        }
    }

    impl Default for Platform {
        fn default() -> Self {
            Self::new()
        }
    }

    impl super::HotkeyBackend for Platform {
        type Handle = Handle;

        fn register(
            &mut self,
            chord: HotkeyChord,
            handler: HotkeyHandler,
        ) -> Result<Self::Handle, HotkeyError> {
            if objc2::MainThreadMarker::new().is_none() {
                return Err(HotkeyError::NotMainThread);
            }
            let code = chord.key.carbon_code().ok_or_else(|| {
                HotkeyError::Invalid(super::ChordError::UnsupportedKey(chord.key.label()))
            })?;
            let mut modifiers = 0u32;
            if chord.command {
                modifiers |= CMD_KEY;
            }
            if chord.shift {
                modifiers |= SHIFT_KEY;
            }
            if chord.option {
                modifiers |= OPTION_KEY;
            }
            if chord.control {
                modifiers |= CONTROL_KEY;
            }

            let _guard = registration_lock()
                .lock()
                .map_err(|_| HotkeyError::Carbon {
                    call: "hotkey registration lock",
                    status: -1,
                })?;
            if HANDLER.lock().map(|slot| slot.is_some()).unwrap_or(false) {
                return Err(HotkeyError::Conflict { chord });
            }
            if let Ok(mut slot) = HANDLER.lock() {
                *slot = Some(handler);
            }

            let spec = EventTypeSpec {
                event_class: K_EVENT_CLASS_KEYBOARD,
                event_kind: K_EVENT_HOT_KEY_PRESSED,
            };
            let mut event_handler: EventHandlerRef = ptr::null_mut();
            let status = unsafe {
                InstallEventHandler(
                    GetApplicationEventTarget(),
                    Some(hotkey_event_handler),
                    1,
                    &spec,
                    ptr::null_mut(),
                    &mut event_handler,
                )
            };
            if status != 0 {
                clear_handler();
                return Err(HotkeyError::Carbon {
                    call: "InstallEventHandler",
                    status,
                });
            }

            let mut hotkey: EventHotKeyRef = ptr::null_mut();
            let id = EventHotKeyID {
                signature: SIGNATURE,
                id: 1,
            };
            let status = unsafe {
                RegisterEventHotKey(
                    code,
                    modifiers,
                    id,
                    GetApplicationEventTarget(),
                    0,
                    &mut hotkey,
                )
            };
            if status != 0 {
                unsafe {
                    RemoveEventHandler(event_handler);
                }
                clear_handler();
                return if status == EVENT_HOT_KEY_EXISTS_ERR {
                    Err(HotkeyError::Conflict { chord })
                } else {
                    Err(HotkeyError::Carbon {
                        call: "RegisterEventHotKey",
                        status,
                    })
                };
            }

            Ok(Handle {
                hotkey: hotkey as usize,
                event_handler: event_handler as usize,
            })
        }

        fn unregister(&mut self, handle: Self::Handle) {
            let _guard = registration_lock().lock();
            unsafe {
                UnregisterEventHotKey(handle.hotkey as EventHotKeyRef);
                RemoveEventHandler(handle.event_handler as EventHandlerRef);
            }
            clear_handler();
        }
    }

    // `kEventClassKeyboard` is the class for hotkey presses; `kEventKind` 5 is
    // `kEventHotKeyPressed` (CarbonEvents.h).

    fn clear_handler() {
        if let Ok(mut slot) = HANDLER.lock() {
            *slot = None;
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    //! Honest stub: the hotkey is a macOS/Carbon feature. The manager's state
    //! machine is still compiled and tested, but a real registration cannot
    //! exist here and says so.

    use super::{HotkeyChord, HotkeyError, HotkeyHandler};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Handle(());

    pub struct Platform {
        _private: (),
    }

    impl Platform {
        pub fn new() -> Self {
            Self { _private: () }
        }
    }

    impl Default for Platform {
        fn default() -> Self {
            Self::new()
        }
    }

    impl super::HotkeyBackend for Platform {
        type Handle = Handle;

        fn register(
            &mut self,
            _chord: HotkeyChord,
            _handler: HotkeyHandler,
        ) -> Result<Self::Handle, HotkeyError> {
            Err(HotkeyError::Unsupported)
        }

        fn unregister(&mut self, _handle: Self::Handle) {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    fn chord(text: &str) -> HotkeyChord {
        HotkeyChord::parse(text).expect("chord parses")
    }

    #[test]
    fn the_default_is_command_shift_space() {
        let default = HotkeyChord::default();
        // Apple's modifier order is ⌃⌥⇧⌘ (the prose in the workorder writes the
        // chord as ⌘⇧Space; both spell the same registration).
        assert_eq!(default.label(), "⇧⌘Space");
        assert_eq!(default.canonical(), "cmd+shift+space");
        assert_eq!(reserved_by_system(&default), None);
        assert_eq!(reserved_by_app(&default), None);
        assert_eq!(advisory_collision(&default), None);
    }

    #[test]
    fn the_settings_grammar_accepts_tokens_spaces_and_glyphs() {
        for text in [
            "cmd+shift+space",
            "⌘⇧Space",
            "CMD + SHIFT + SPACE",
            "  command shift space  ",
            "Shift+Cmd+Space",
        ] {
            assert_eq!(chord(text), DEFAULT_HOTKEY, "{text:?}");
        }
        assert_eq!(
            chord("⌘⇧R"),
            HotkeyChord {
                key: HotkeyKey::Letter('R'),
                command: true,
                shift: true,
                option: false,
                control: false,
            }
        );
        assert_eq!(chord("ctrl+opt+space").label(), "⌃⌥Space");
    }

    #[test]
    fn display_and_canonical_round_trip() {
        for text in [
            "cmd+shift+space",
            "cmd+shift+r",
            "ctrl+opt+a",
            "ctrl+shift+z",
        ] {
            let parsed = chord(text);
            assert_eq!(HotkeyChord::parse(&parsed.label()).unwrap(), parsed);
            assert_eq!(HotkeyChord::parse(&parsed.canonical()).unwrap(), parsed);
        }
    }

    #[test]
    fn shift_alone_is_not_a_global_chord() {
        let err = HotkeyChord::parse("shift+r").expect_err("shift-only is refused");
        assert_eq!(err, ChordError::ShiftOnly);
        assert!(err.to_string().contains("Shift alone"));
    }

    #[test]
    fn malformed_chords_report_the_grammar() {
        assert_eq!(HotkeyChord::parse(""), Err(ChordError::Empty));
        assert_eq!(HotkeyChord::parse("cmd+shift"), Err(ChordError::MissingKey));
        assert!(matches!(
            HotkeyChord::parse("cmd+r+s"),
            Err(ChordError::MultipleKeys(..))
        ));
        assert!(matches!(
            HotkeyChord::parse("hyper+r"),
            Err(ChordError::UnknownModifier(_))
        ));
        assert!(matches!(
            HotkeyChord::parse("cmd+escape"),
            Err(ChordError::UnsupportedKey(_))
        ));
        assert!(matches!(
            HotkeyChord::parse("cmd+1"),
            Err(ChordError::UnsupportedKey(_))
        ));
        assert_eq!(HotkeyChord::parse("r"), Err(ChordError::NoModifier));
    }

    #[test]
    fn system_and_app_reserved_chords_are_named() {
        assert_eq!(
            reserved_by_system(&chord("cmd+space")),
            Some("Spotlight search")
        );
        assert_eq!(
            reserved_by_system(&chord("ctrl+cmd+space")),
            Some("Emoji & Symbols")
        );
        assert_eq!(reserved_by_system(&chord("cmd+shift+q")), Some("Log Out"));
        assert_eq!(
            reserved_by_app(&chord("cmd+q")),
            Some("Suramak's Quit (⌘Q)")
        );
        assert_eq!(
            reserved_by_app(&chord("cmd+w")),
            Some("Suramak's Close Window (⌘W)")
        );
        // The chord the workorder offered as the alternative default really is
        // shadowed by two mainstream apps — the advisory says which.
        let reader = advisory_collision(&chord("cmd+shift+r")).expect("advisory");
        assert!(reader.contains("Safari's Reader"), "{reader}");
        assert!(reader.contains("Chrome"), "{reader}");
    }

    // ---- the registration state machine, with a fake Carbon ----------------

    #[derive(Default)]
    struct FakeBackend {
        log: Arc<Mutex<Vec<String>>>,
        next: u64,
        /// Return `Conflict` when this chord is registered.
        conflict: Option<HotkeyChord>,
        /// The most recently stored handler, for `press()` from a test clone.
        handler: Arc<Mutex<Option<HotkeyHandler>>>,
        unregisters: Arc<AtomicUsize>,
    }

    impl FakeBackend {
        fn with_conflict(conflict: HotkeyChord) -> Self {
            Self {
                conflict: Some(conflict),
                ..Self::default()
            }
        }
    }

    impl HotkeyBackend for FakeBackend {
        type Handle = u64;

        fn register(
            &mut self,
            chord: HotkeyChord,
            handler: HotkeyHandler,
        ) -> Result<Self::Handle, HotkeyError> {
            self.log
                .lock()
                .unwrap()
                .push(format!("register {}", chord.canonical()));
            if self.conflict == Some(chord) {
                return Err(HotkeyError::Conflict { chord });
            }
            self.next += 1;
            *self.handler.lock().unwrap() = Some(handler);
            Ok(self.next)
        }

        fn unregister(&mut self, handle: Self::Handle) {
            self.log
                .lock()
                .unwrap()
                .push(format!("unregister {handle}"));
            self.unregisters.fetch_add(1, Ordering::SeqCst);
            *self.handler.lock().unwrap() = None;
        }
    }

    fn fake() -> (HotkeyRegistration<FakeBackend>, Arc<Mutex<Vec<String>>>) {
        let backend = FakeBackend::default();
        let log = Arc::clone(&backend.log);
        (
            HotkeyRegistration::with_backend(backend, Arc::new(|| {})),
            log,
        )
    }

    #[test]
    fn a_chord_can_be_registered_and_changed_with_a_clean_unregister_first() {
        let (mut registration, log) = fake();
        assert_eq!(registration.chord(), None);
        registration.change(chord("cmd+shift+space")).unwrap();
        assert_eq!(registration.chord(), Some(chord("cmd+shift+space")));
        assert_eq!(log.lock().unwrap().as_slice(), ["register cmd+shift+space"]);

        registration.change(chord("cmd+shift+r")).unwrap();
        assert_eq!(registration.chord(), Some(chord("cmd+shift+r")));
        let log = log.lock().unwrap().clone();
        assert_eq!(
            log,
            [
                "register cmd+shift+space",
                "unregister 1",
                "register cmd+shift+r"
            ],
            "the old chord is unregistered before the new one is attempted"
        );
    }

    #[test]
    fn changing_to_the_same_chord_is_a_no_op() {
        let (mut registration, log) = fake();
        let chord = chord("cmd+shift+space");
        registration.change(chord).unwrap();
        log.lock().unwrap().clear();
        registration.change(chord).unwrap();
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn a_refused_change_restores_the_previous_chord() {
        let backend = FakeBackend::with_conflict(chord("cmd+shift+k"));
        let log = Arc::clone(&backend.log);
        let mut registration = HotkeyRegistration::with_backend(backend, Arc::new(|| {}));
        registration.change(chord("cmd+shift+space")).unwrap();
        log.lock().unwrap().clear();

        let err = registration
            .change(chord("cmd+shift+k"))
            .expect_err("the conflicting chord is refused");
        assert_eq!(
            err,
            HotkeyError::Conflict {
                chord: chord("cmd+shift+k")
            }
        );
        assert_eq!(
            registration.chord(),
            Some(chord("cmd+shift+space")),
            "the previous chord is re-registered"
        );
        assert_eq!(
            log.lock().unwrap().as_slice(),
            [
                "unregister 1",
                "register cmd+shift+k",
                "register cmd+shift+space"
            ]
        );
    }

    #[test]
    fn system_and_app_reserved_chords_never_reach_the_backend() {
        let (mut registration, log) = fake();
        let err = registration.change(chord("cmd+space")).unwrap_err();
        assert!(matches!(err, HotkeyError::Reserved { .. }));
        let err = registration.change(chord("cmd+q")).unwrap_err();
        assert!(matches!(err, HotkeyError::AppReserved { .. }));
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn unregister_releases_the_backend_once_and_drop_is_idempotent() {
        let backend = FakeBackend::default();
        let unregisters = Arc::clone(&backend.unregisters);
        let log = Arc::clone(&backend.log);
        {
            let mut registration = HotkeyRegistration::with_backend(backend, Arc::new(|| {}));
            registration.change(chord("cmd+shift+space")).unwrap();
            registration.unregister();
            assert_eq!(unregisters.load(Ordering::SeqCst), 1);
            // The value has been consumed; nothing else can run.
        }
        assert_eq!(unregisters.load(Ordering::SeqCst), 1);
        assert_eq!(
            log.lock().unwrap().as_slice().last().unwrap(),
            "unregister 1"
        );
    }

    #[test]
    fn dropping_a_live_registration_unregisters_it() {
        let backend = FakeBackend::default();
        let unregisters = Arc::clone(&backend.unregisters);
        {
            let mut registration = HotkeyRegistration::with_backend(backend, Arc::new(|| {}));
            registration.change(chord("cmd+shift+space")).unwrap();
            assert_eq!(unregisters.load(Ordering::SeqCst), 0);
        }
        assert_eq!(unregisters.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_handler_is_the_one_the_caller_passed_and_press_fires_it() {
        let backend = FakeBackend::default();
        let slot = Arc::clone(&backend.handler);
        let fired = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&fired);
        let mut registration = HotkeyRegistration::with_backend(
            backend,
            Arc::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }),
        );
        registration.change(chord("cmd+shift+space")).unwrap();
        // Fire the exact closure the backend stored (the Carbon handler does
        // the same thing from its C callback).
        let handler = slot.lock().unwrap().clone().expect("handler registered");
        handler();
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        assert_eq!(registration.chord(), Some(chord("cmd+shift+space")));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn registration_reports_unsupported_off_macos() {
        match register_hotkey(chord("cmd+shift+space"), Arc::new(|| {})) {
            Ok(_) => panic!("no Carbon off macOS"),
            Err(err) => {
                assert_eq!(err, HotkeyError::Unsupported);
                assert!(err.to_string().contains("macOS"));
            }
        }
    }
}
