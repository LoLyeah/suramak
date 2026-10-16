//! M12: **in-app updates** (Sparkle 2) — macOS-only, behind the default-OFF
//! `updates` cargo feature.
//!
//! What this module owns (per `docs/WORKORDER-M12.md`):
//!
//! - **Starting the updater** on the AppKit main thread at boot, after the
//!   window is up (`App::resumed` calls [`Updater::start`]). The update policy
//!   is ours: `settings.updates.auto_check` ("Automatic updates", ON by
//!   default) is applied to Sparkle explicitly at start and after every
//!   settings save — Sparkle never guesses a first-run policy.
//! - **Event callbacks into our logging**: every Sparkle notification
//!   (`UpdateEvent`) is logged (milestones at info, failures at warn, the rest
//!   at debug) — the owner's update cycle is diagnosable from the app log.
//! - **A "Check for Updates…" app-menu item** (Sparkle's programmatic-setup
//!   target/action pattern). This is the one piece the `sparkle-updater` crate
//!   does not wrap, so it is a minimal own objc2 class here: an `NSObject`
//!   subclass whose `checkForUpdates:` calls the crate's user-initiated check
//!   and whose `validateMenuItem:` mirrors Sparkle's own controller
//!   (`canCheckForUpdates` — the menu item greys out during a check).
//! - **The gentle-reminders and relaunch hooks** (observational only): the
//!   reminders delegate logs and returns the default decisions (Sparkle keeps
//!   its native reminder UI — `should_show_scheduled_update` stays at the
//!   trait's `true` default); the relaunch hook logs and resumes the
//!   continuation immediately (the app has no extra pre-relaunch saving beyond
//!   its own quit lifecycle, and a live recording is guarded by the M9e quit
//!   confirm).
//!
//! Why the crate (the spike decision): `sparkle-updater` 0.1.0 is objc2
//! bindings for **Sparkle 2.9.6** — the exact subset needed (start, events,
//! reminders, relaunch, the automatic-check preference, `canCheckForUpdates`)
//! with no Tauri/GPUI/Tokio dependency. Its build script needs
//! `SPARKLE_FRAMEWORK_PATH` pointing at a directory containing
//! `Sparkle.framework` (fetched pinned by `scripts/fetch-sparkle.sh`), and its
//! README defines the bundling contract this repo implements: the framework in
//! `Contents/Frameworks`, the `@executable_path/../Frameworks` rpath (emitted
//! by `build.rs`) and nested code signing (`scripts/make-app.sh`). A hand-rolled
//! binding to `SPUStandardUpdaterController` would have to re-create all of
//! that surface; the crate is the smaller, reviewed path.
//!
//! Structure (the M10B gating pattern): the module **always exists with the
//! same API shape**. Without the feature (or off-macOS) it is an honest stub —
//! [`compiled()`] is `false`, [`Updater::start`] logs why and does nothing — so
//! default builds are unchanged and every pure helper below is unit-tested on
//! Linux. The `imp` module is the single place the crate/objc2 are touched and
//! is only compiled for `cfg(all(feature = "updates", target_os = "macos"))`.
//!
//! Boundaries (honest): the entire update cycle — check → prompt → download →
//! install → relaunch — is **owner-verify** (USER_SMOKE §32): none of it can
//! run on Linux. What Linux verifies is everything above: the stub, the
//! settings round-trips, the plist/pin/workflow contracts in the tests below,
//! and a `cargo check --target aarch64-apple-darwin` of the verbatim module
//! against the real crate (the throwaway `build/probe-updates` probe — the
//! M10B probe pattern). The macOS CI and release builds compile the real thing.

/// Whether this build carries the in-app updater: the `updates` cargo feature
/// **and** a macOS target (the crate links `Sparkle.framework` and the menu
/// item is AppKit — neither exists anywhere else).
pub const fn compiled() -> bool {
    cfg!(all(feature = "updates", target_os = "macos"))
}

/// The packaging contract, compiled on every platform: the macOS `imp` module
/// runs these checks at start, and the tests below use them on Linux. In a
/// stub build (no feature / off-macOS) only the tests reference them, hence
/// the scoped `allow(dead_code)` — the macOS build still checks every use.
#[cfg_attr(not(all(feature = "updates", target_os = "macos")), allow(dead_code))]
mod contract {
    /// The appcast this app follows (`SUFeedURL` in `packaging/Info.plist`) —
    /// the newest GitHub release's `appcast.xml` asset. Kept here as the
    /// single expectation the bundle is checked against at start.
    pub(super) const FEED_URL: &str =
        "https://github.com/LoLyeah/suramak/releases/latest/download/appcast.xml";

    /// The shipped placeholder value of `SUPublicEDKey` (see
    /// `packaging/Info.plist` and `docs/reports/M12.md` §Owner key setup). It
    /// is **not a key**: base64-decoded it is the ASCII marker
    /// `SURAMAK-PLACEHOLDER-ED25519-KEY!` — 32 bytes, so Sparkle's
    /// configuration check accepts its *shape* (an invalid-shaped value would
    /// make Sparkle refuse to start the updater) while it matches no real
    /// signature. The owner replaces it with the public key printed by
    /// Sparkle's `generate_keys`.
    pub(super) const PLACEHOLDER_PUBLIC_ED_KEY: &str =
        "U1VSQU1BSy1QTEFDRUhPTERFUi1FRDI1NTE5LUtFWSE=";

    /// The `<string>` value of a top-level plist `<key>`, when the value is the
    /// very next element (a simple key/string pair — every key this module
    /// reads is one). Deliberately dependency-free and pure: the same function
    /// backs the boot-time bundle check on macOS and the packaging tests on
    /// Linux.
    pub(super) fn plist_string(plist: &str, key: &str) -> Option<String> {
        let needle = format!("<key>{key}</key>");
        let after = plist.get(plist.find(&needle)? + needle.len()..)?;
        let open = after.find('<')?;
        if !after[open..].starts_with("<string>") {
            return None; // not a key/string pair (array, dict, …) — not ours
        }
        let value_start = open + "<string>".len();
        let end = after[value_start..].find("</string>")?;
        Some(after[value_start..value_start + end].trim().to_string())
    }

    /// Whether the plist still carries the shipped placeholder `SUPublicEDKey`
    /// (true until the owner runs the one-time key setup).
    pub(super) fn public_ed_key_is_placeholder(plist: &str) -> bool {
        plist_string(plist, "SUPublicEDKey").is_some_and(|value| value == PLACEHOLDER_PUBLIC_ED_KEY)
    }
}

// ---------------------------------------------------------------------------
// The updater handle — real on macOS + `updates`, an honest stub elsewhere
// ---------------------------------------------------------------------------

#[cfg(all(feature = "updates", target_os = "macos"))]
pub struct Updater {
    /// `Some` while Sparkle runs; `None` when this run has no updater (not an
    /// app bundle, refused configuration, off the main thread) — the reasons
    /// are logged at start.
    handle: Option<Box<imp::Handle>>,
}

#[cfg(all(feature = "updates", target_os = "macos"))]
impl Updater {
    /// Starts the updater. **Main thread only** (the crate and AppKit both
    /// require it — `App::resumed` calls this after the window is up).
    ///
    /// `automatic_checks` is the persisted "Automatic updates" setting; it is
    /// applied to Sparkle explicitly (our `settings.toml` is the source of
    /// truth, not Sparkle's own user defaults).
    ///
    /// Never panics and never fails the app: an updater that cannot start
    /// (not inside an app bundle, Sparkle refusing the configuration, no main
    /// thread) is logged and the app simply runs without updates.
    pub fn start(automatic_checks: bool) -> Self {
        Self {
            handle: imp::start(automatic_checks),
        }
    }

    /// Applies the "Automatic updates" policy to the running updater. A no-op
    /// when the updater is off; skips Sparkle's setter when the value has not
    /// changed (the setter writes user defaults — no per-frame disk churn).
    pub fn set_automatic_checks(&mut self, enabled: bool) {
        let Some(handle) = self.handle.as_mut() else {
            return; // off in this run — nothing to update
        };
        imp::set_automatic_checks(handle, enabled);
    }
}

#[cfg(not(all(feature = "updates", target_os = "macos")))]
pub struct Updater {
    /// The stub carries no state: without the feature (or off-macOS) there is
    /// nothing to run, and the module keeps the same API shape so `main.rs`
    /// needs no cfg.
    _private: (),
}

#[cfg(not(all(feature = "updates", target_os = "macos")))]
impl Updater {
    /// The stub: logs why the updater is off and does nothing. Same signature
    /// as the real [`Updater::start`] — the only difference is the build.
    pub fn start(_automatic_checks: bool) -> Self {
        tracing::info!(
            "updates: no updater in this build — the `updates` feature is macOS-only and OFF \
             by default (default builds are unchanged)"
        );
        Self { _private: () }
    }

    /// The stub: nothing to apply (no updater exists in this build).
    pub fn set_automatic_checks(&mut self, _enabled: bool) {}
}

// ---------------------------------------------------------------------------
// The macOS implementation — the only place the crate and objc2 are touched
// ---------------------------------------------------------------------------

#[cfg(all(feature = "updates", target_os = "macos"))]
mod imp {
    use std::rc::Rc;

    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker};
    use objc2_app_kit::{NSApplication, NSMenuItem};
    use objc2_foundation::{ns_string, NSObject};
    use sparkle_updater::events::{UpdateEvent, UpdateInfo, UserUpdateState};
    use sparkle_updater::{
        GentleReminders, RelaunchContinuation, RelaunchHandler, SparkleUpdater, UpdaterConfig,
    };

    use super::contract::{plist_string, public_ed_key_is_placeholder, FEED_URL};

    pub(super) struct Handle {
        updater: Rc<SparkleUpdater>,
        /// The "Check for Updates…" menu item's target. AppKit does **not**
        /// retain menu-item targets, so the handle must keep it alive; it in
        /// turn keeps the updater alive through its ivars.
        _menu_target: Retained<MenuTarget>,
        /// The automatic-checks value last applied to Sparkle (skip redundant
        /// user-defaults writes).
        automatic: bool,
    }

    /// The bundle's `Contents/Info.plist` (`<app>/Contents/MacOS/suramak` →
    /// `<app>/Contents/Info.plist`), when readable.
    fn bundled_info_plist() -> Option<String> {
        let exe = std::env::current_exe().ok()?;
        let contents = exe.parent()?.parent()?;
        std::fs::read_to_string(contents.join("Info.plist")).ok()
    }

    /// One boot-time packaging check, independent of Sparkle: the bundle's
    /// `SUFeedURL` must be the feed this build expects, and the public key must
    /// no longer be the shipped placeholder. Both are loud log lines (the app
    /// keeps running — a packaging mistake must not brick it).
    fn check_bundle_plist() {
        let Some(plist) = bundled_info_plist() else {
            tracing::debug!(
                "updates: could not read the bundle's Info.plist — skipping the checks"
            );
            return;
        };
        match plist_string(&plist, "SUFeedURL") {
            Some(url) if url == FEED_URL => {}
            Some(url) => tracing::warn!(
                expected = FEED_URL,
                found = %url,
                "updates: SUFeedURL differs from the feed this build expects"
            ),
            None => tracing::warn!(
                expected = FEED_URL,
                "updates: SUFeedURL is missing from the bundle's Info.plist — updates cannot check"
            ),
        }
        if public_ed_key_is_placeholder(&plist) {
            tracing::warn!(
                "updates: SUPublicEDKey is still the shipped placeholder — updates will not \
                 verify until the owner sets the real key (docs/reports/M12.md, \"Owner key \
                 setup\")"
            );
        }
    }

    /// Logs one Sparkle notification. Callbacks run synchronously on the main
    /// thread and must stay brief and panic-free — formatting only.
    fn log_event(event: &UpdateEvent) {
        match event {
            UpdateEvent::DidFindValidUpdate(info) => {
                tracing::info!(version = %info.version, "updates: a new version is available")
            }
            UpdateEvent::DidNotFindUpdate(info) => tracing::info!(
                reason = ?info.reason,
                user_initiated = info.user_initiated,
                "updates: no update available"
            ),
            UpdateEvent::DidAbortWithError(payload) => tracing::warn!(
                message = %payload.message,
                code = payload.code,
                domain = %payload.domain,
                "updates: the update check aborted"
            ),
            UpdateEvent::FailedToDownloadUpdate(info) => tracing::warn!(
                version = %info.version,
                message = %info.error.message,
                "updates: the update download failed"
            ),
            UpdateEvent::WillDownloadUpdate(info) => {
                tracing::info!(version = %info.version, "updates: downloading the update")
            }
            UpdateEvent::DidDownloadUpdate(info) => {
                tracing::info!(version = %info.version, "updates: the update is downloaded")
            }
            UpdateEvent::WillInstallUpdate(info) => {
                tracing::info!(version = %info.version, "updates: installing the update")
            }
            UpdateEvent::WillInstallUpdateOnQuit(info) => tracing::info!(
                version = %info.version,
                "updates: the update will install when the app quits"
            ),
            UpdateEvent::WillRelaunchApplication => {
                tracing::info!("updates: relaunching the app")
            }
            UpdateEvent::UserDidMakeChoice(info) => tracing::info!(
                choice = %info.choice,
                version = %info.version,
                stage = %info.stage,
                "updates: user choice"
            ),
            UpdateEvent::UserDidCancelDownload => {
                tracing::info!("updates: the user cancelled the download")
            }
            UpdateEvent::DidFinishUpdateCycle(info) => tracing::debug!(
                update_check = %info.update_check,
                failed = info.error.is_some(),
                "updates: the update cycle finished"
            ),
            UpdateEvent::DidFinishLoadingAppcast => {
                tracing::debug!("updates: the appcast is loaded")
            }
            UpdateEvent::WillExtractUpdate(info) => {
                tracing::debug!(version = %info.version, "updates: extracting the update")
            }
            UpdateEvent::DidExtractUpdate(info) => {
                tracing::debug!(version = %info.version, "updates: the update is extracted")
            }
            UpdateEvent::WillScheduleUpdateCheck(info) => {
                tracing::debug!(delay_s = info.delay, "updates: the next check is scheduled")
            }
            UpdateEvent::WillNotScheduleUpdateCheck => {
                tracing::debug!("updates: no further check is scheduled (automatic checks are off)")
            }
            // `UpdateEvent` is #[non_exhaustive]: a future crate/Sparkle may add
            // events. Log the stable name so nothing is ever swallowed.
            _ => tracing::debug!(event = event.name(), "updates: event"),
        }
    }

    /// The gentle-reminders delegate: **observational only** — every decision
    /// stays at the trait's default (`should_show_scheduled_update` returns
    /// `true`, so Sparkle presents its own reminder UI for scheduled checks).
    struct Reminders;

    impl GentleReminders for Reminders {
        fn will_show_update(
            &self,
            handled_by_sparkle: bool,
            update: &UpdateInfo,
            state: UserUpdateState,
        ) {
            tracing::info!(
                handled_by_sparkle,
                version = %update.version,
                user_initiated = state.user_initiated,
                stage = ?state.stage,
                "updates: showing an update"
            );
        }

        fn did_receive_user_attention(&self, update: &UpdateInfo) {
            tracing::debug!(version = %update.version, "updates: the user saw the reminder");
        }

        fn will_finish_update_session(&self) {
            tracing::debug!("updates: the update session finished");
        }
    }

    /// The relaunch hook: Sparkle offers the host a chance to save work before
    /// its install relaunch. Suramak has nothing extra to defer here — its
    /// quit lifecycle owns saving (and a live recording is guarded by the M9e
    /// quit confirm) — so the continuation is resumed immediately, which is the
    /// documented "proceed as if no handler were installed" path.
    fn relaunch_handler() -> RelaunchHandler {
        Rc::new(|update: UpdateInfo, continuation: RelaunchContinuation| {
            tracing::info!(
                version = %update.version,
                "updates: Sparkle is ready to relaunch — resuming immediately (the app saves \
                 through its own quit lifecycle)"
            );
            match MainThreadMarker::new() {
                Some(mtm) => continuation.resume(mtm),
                None => tracing::warn!(
                    "updates: the relaunch handler ran off the main thread — the relaunch stays \
                     postponed"
                ),
            }
        })
    }

    /// The "Check for Updates…" menu item's target: the one piece the
    /// `sparkle-updater` crate does not wrap, so it is a minimal own class.
    ///
    /// `checkForUpdates:`/`validateMenuItem:` mirror Sparkle's own
    /// `SPUStandardUpdaterController` (the programmatic-setup target/action
    /// pattern); the ivars hold the updater so the action reaches the same
    /// instance the app started.
    struct MenuTargetIvars {
        updater: Rc<SparkleUpdater>,
    }

    define_class!(
        // SAFETY: `NSObject` has no subclassing requirements; `MenuTarget`
        // does not implement `Drop`, and its ivars are a main-thread-only `Rc`
        // (the class is therefore !Send/!Sync, which is what AppKit wants).
        #[unsafe(super(NSObject))]
        #[name = "SuramakUpdatesMenuTarget"]
        #[ivars = MenuTargetIvars]
        struct MenuTarget;

        impl MenuTarget {
            /// The menu item's action — Sparkle's standard user-initiated check.
            #[unsafe(method(checkForUpdates:))]
            fn check_for_updates(&self, _sender: Option<&AnyObject>) {
                if let Err(err) = self.ivars().updater.check_for_updates() {
                    tracing::warn!(error = %err, "updates: the check request was refused");
                }
            }

            /// Menu-item validation, exactly like Sparkle's controller: enabled
            /// while `canCheckForUpdates` (no check/install session running).
            /// `unwrap_or(true)` keeps the item usable if the query fails.
            #[unsafe(method(validateMenuItem:))]
            fn validate_menu_item(&self, _item: Option<&AnyObject>) -> bool {
                self.ivars().updater.can_check_for_updates().unwrap_or(true)
            }
        }
    );

    /// Creates the menu target (alloc + ivars + `init`).
    fn make_menu_target(updater: Rc<SparkleUpdater>) -> Retained<MenuTarget> {
        let target = MenuTarget::alloc().set_ivars(MenuTargetIvars { updater });
        // SAFETY: `NSObject`'s `init` is the designated initializer; the ivars
        // were set before it runs.
        unsafe { msg_send![super(target), init] }
    }

    /// Adds "Check for Updates…" to the app menu (right after "About …" — the
    /// standard position). winit builds the menu bar with the app menu as the
    /// first item (verified against the pinned fork's
    /// `platform_impl/macos/menu.rs`); an unexpectedly shaped menu is logged
    /// and skipped, never patched blindly.
    fn install_menu_item(mtm: MainThreadMarker, target: &MenuTarget) {
        let app = NSApplication::sharedApplication(mtm);
        let Some(app_menu) = app
            .mainMenu()
            .and_then(|menu| menu.itemAtIndex(0))
            .and_then(|item| item.submenu())
        else {
            tracing::warn!("updates: no app menu to add Check for Updates… to — skipping");
            return;
        };
        // Idempotent: never add a second item with the same action.
        for index in 0..app_menu.numberOfItems() {
            if app_menu
                .itemAtIndex(index)
                .is_some_and(|item| item.action() == Some(sel!(checkForUpdates:)))
            {
                tracing::debug!("updates: Check for Updates… is already in the app menu");
                return;
            }
        }
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                ns_string!("Check for Updates…"),
                Some(sel!(checkForUpdates:)),
                ns_string!(""),
            )
        };
        // SAFETY: `target` is our main-thread object, kept alive by the handle
        // for the app's lifetime; AppKit does not retain menu-item targets, and
        // the item never outlives the handle.
        unsafe {
            item.setTarget(Some(&*(target as *const MenuTarget).cast::<AnyObject>()));
        }
        if app_menu.numberOfItems() >= 1 {
            // After "About <app>".
            app_menu.insertItem_atIndex(&item, 1);
        } else {
            app_menu.addItem(&item);
        }
        tracing::info!("updates: added \"Check for Updates…\" to the app menu");
    }

    /// Starts Sparkle. Never fails the app: every failure mode is a log line
    /// and `None`.
    pub(super) fn start(automatic_checks: bool) -> Option<Box<Handle>> {
        let Some(mtm) = MainThreadMarker::new() else {
            tracing::warn!("updates: not on the AppKit main thread — the updater is off");
            return None;
        };
        let config = UpdaterConfig {
            event_callback: Some(Rc::new(|event| log_event(&event))),
            relaunch_handler: Some(relaunch_handler()),
            gentle_reminders: Some(Rc::new(Reminders)),
        };
        let updater = match SparkleUpdater::new(mtm, config) {
            Ok(Some(updater)) => Rc::new(updater),
            Ok(None) => {
                // The crate logs its own "not running inside a valid macOS
                // bundle" warning; this line ties it to the app's log.
                tracing::info!(
                    "updates: not an app bundle — the updater is off (packaged builds only)"
                );
                return None;
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "updates: Sparkle refused to start — the app runs without updates"
                );
                return None;
            }
        };

        // Our settings.toml is the source of truth for the policy: apply it
        // explicitly (the crate's README asks hosts to do so), and never
        // auto-download without the user's consent — Sparkle's standard UI
        // asks before installing.
        if let Err(err) = updater.set_automatically_checks_for_updates(automatic_checks) {
            tracing::warn!(error = %err, "updates: could not apply the automatic-checks policy");
        }
        if let Err(err) = updater.set_automatically_downloads_updates(false) {
            tracing::warn!(error = %err, "updates: could not pin manual downloads");
        }
        check_bundle_plist();
        let version = updater
            .current_version()
            .unwrap_or_else(|_| "unknown".to_string());
        let feed = updater
            .feed_url()
            .ok()
            .flatten()
            .unwrap_or_else(|| "unset".to_string());
        tracing::info!(
            version = %version,
            feed = %feed,
            automatic_checks,
            "updates: Sparkle started"
        );

        let menu_target = make_menu_target(Rc::clone(&updater));
        install_menu_item(mtm, &menu_target);
        Some(Box::new(Handle {
            updater,
            _menu_target: menu_target,
            automatic: automatic_checks,
        }))
    }

    /// Applies the automatic-checks policy to a running updater.
    pub(super) fn set_automatic_checks(handle: &mut Handle, enabled: bool) {
        if handle.automatic == enabled {
            return; // unchanged — skip Sparkle's user-defaults write
        }
        match handle.updater.set_automatically_checks_for_updates(enabled) {
            Ok(()) => {
                handle.automatic = enabled;
                tracing::info!(
                    automatic_checks = enabled,
                    "updates: automatic checks updated"
                );
            }
            Err(err) => {
                tracing::warn!(error = %err, "updates: could not change the automatic-checks setting")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — everything here runs on Linux (the module's pure bits and the
// repo-file contracts). The updater runtime itself is owner-verify (§32).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::contract::*;
    use super::*;

    /// The repo root, from this crate's manifest dir (`crates/suramak-ui`).
    fn repo_root() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn repo_file(relative: &str) -> String {
        let path = repo_root().join(relative);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()))
    }

    // ---- the pure plist reader ----------------------------------------------

    #[test]
    fn plist_string_reads_simple_key_string_pairs() {
        let plist = r#"<?xml version="1.0"?>
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>Suramak</string>
	<key>SUFeedURL</key>
	<string>https://example.test/appcast.xml</string>
	<key>NSCameraUsageDescription</key>
	<string>  padded  </string>
</dict>
</plist>"#;
        assert_eq!(
            plist_string(plist, "SUFeedURL").as_deref(),
            Some("https://example.test/appcast.xml")
        );
        // The value is trimmed (a pretty-printed plist may wrap it).
        assert_eq!(
            plist_string(plist, "NSCameraUsageDescription").as_deref(),
            Some("padded")
        );
        assert_eq!(
            plist_string(plist, "CFBundleName").as_deref(),
            Some("Suramak")
        );
        // Missing key, and a key whose value is not a string → None (never a
        // neighbouring value).
        assert_eq!(plist_string(plist, "NoSuchKey"), None);
        let array_valued =
            "<key>SUAllowedChannels</key>\n\t<array>\n\t\t<string>x</string>\n\t</array>";
        assert_eq!(plist_string(array_valued, "SUAllowedChannels"), None);
    }

    // ---- the shipped Info.plist contract ------------------------------------

    #[test]
    fn the_shipped_plist_pins_the_feed_url() {
        let plist = repo_file("packaging/Info.plist");
        assert_eq!(
            plist_string(&plist, "SUFeedURL").as_deref(),
            Some(FEED_URL),
            "SUFeedURL must be the GitHub Releases appcast the release pipeline attaches"
        );
    }

    #[test]
    fn the_shipped_plist_carries_the_marked_placeholder_public_key() {
        use base64::Engine as _;

        let plist = repo_file("packaging/Info.plist");
        let key = plist_string(&plist, "SUPublicEDKey")
            .expect("SUPublicEDKey must be present (the placeholder until the owner's key setup)");
        assert_eq!(key, PLACEHOLDER_PUBLIC_ED_KEY);
        assert!(public_ed_key_is_placeholder(&plist));

        // Sparkle's configuration check accepts a key only when it base64-
        // decodes to exactly 32 bytes (SUPublicKeys: "Don't allow invalid EdDSA
        // public keys"); an invalid-shaped value makes the updater refuse to
        // start. The placeholder must decode — and decode to the marker text,
        // so nobody can mistake it for a real key.
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(key)
            .expect("the placeholder must be valid base64");
        assert_eq!(decoded.len(), 32, "an Ed25519 public key is 32 bytes");
        assert_eq!(
            String::from_utf8(decoded).expect("the placeholder decodes to ASCII"),
            "SURAMAK-PLACEHOLDER-ED25519-KEY!"
        );
    }

    // ---- the pinned framework fetch -----------------------------------------

    /// The real archive the pin was taken from, verified on the VPS
    /// (2026-10-16): `sha256sum Sparkle-2.9.6.tar.xz` →
    /// `52bf9e88…e192`, 15,554,568 bytes, from the official 2.9.6 release.
    /// The script re-verifies this hash on every download.
    #[test]
    fn fetch_sparkle_pins_a_wellformed_version_url_and_sha256() {
        let script = repo_file("scripts/fetch-sparkle.sh");
        let version = "2.9.6";
        assert!(
            script.contains(&format!("SPARKLE_VERSION=\"{version}\"")),
            "the version pin must be literal"
        );
        assert!(
            script.contains(
                "https://github.com/sparkle-project/Sparkle/releases/download/${SPARKLE_VERSION}/Sparkle-${SPARKLE_VERSION}.tar.xz"
            ),
            "the URL must be built from the pinned version (official releases only)"
        );
        let sha = script
            .lines()
            .find_map(|line| line.strip_prefix("SPARKLE_SHA256=\"")?.strip_suffix('"'))
            .expect("SPARKLE_SHA256 must be pinned");
        assert_eq!(sha.len(), 64, "sha256 is 64 hex chars: {sha}");
        assert!(
            sha.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "sha256 must be lowercase hex: {sha}"
        );
        assert_eq!(
            sha, "52bf9e88cdd972fc0c81501377a880e90d47031bd8ca5462488f843e2609e192",
            "the recorded hash is the one measured on the VPS (never invented)"
        );
        assert!(
            script.contains("sha256sum") || script.contains("shasum"),
            "the script must verify the hash it pins"
        );

        // When the pinned archive is cached (this repo's VPS/CI runs fetch it
        // into build/sparkle/<version>/), verify the recorded hash against the
        // real file — the strongest check this test can make.
        let cached = repo_root()
            .join("build/sparkle")
            .join(version)
            .join(format!("Sparkle-{version}.tar.xz"));
        if cached.is_file() {
            let bytes = std::fs::read(&cached).expect("read the cached archive");
            let digest = {
                use sha2::{Digest as _, Sha256};
                let mut hasher = Sha256::new();
                hasher.update(&bytes);
                format!("{:x}", hasher.finalize())
            };
            assert_eq!(
                digest,
                sha,
                "the cached archive must match the pinned sha256 ({})",
                cached.display()
            );
            println!(
                "verified {} bytes of {} against the pinned sha256",
                bytes.len(),
                cached.display()
            );
        } else {
            println!(
                "note: {} not cached — run scripts/fetch-sparkle.sh to verify the pin \
                 against the real archive",
                cached.display()
            );
        }
    }

    // ---- the bundling + release contracts -----------------------------------

    #[test]
    fn make_app_embeds_and_signs_the_framework_only_when_the_feature_is_on() {
        let script = repo_file("scripts/make-app.sh");
        // The framework comes from the pinned fetch script — never a bare
        // download inside the packaging script.
        assert!(
            script.contains("scripts/fetch-sparkle.sh"),
            "make-app.sh must fetch through the pinned script"
        );
        // The embedding contract (the crate's README): the framework in
        // Contents/Frameworks, symlinks preserved (ditto), signed nested.
        assert!(
            script.contains("Contents/Frameworks"),
            "the framework must be embedded into Contents/Frameworks"
        );
        assert!(
            script.contains("ditto"),
            "frameworks carry symlinks — the copy must preserve them (ditto)"
        );
        assert!(
            script.contains("codesign --force --deep --sign -"),
            "the existing ad-hoc signing flow (which signs nested code) must stay"
        );
        // The embed must happen before the signing line, or the signature
        // would not cover the framework.
        let embed = script.find("Contents/Frameworks").expect("embed");
        let sign = script
            .find("codesign --force --deep --sign -")
            .expect("signing");
        assert!(embed < sign, "the framework must be copied before signing");
        // The rpath lives in build.rs (a build-time link argument); the
        // packaging script must not patch the binary afterwards.
        assert!(
            !script.contains("install_name_tool"),
            "the rpath must be a link-time argument (build.rs), not a post-hoc edit"
        );
        let build_rs = repo_file("crates/suramak-ui/build.rs");
        assert!(
            build_rs.contains("rustc-link-arg-bins")
                && build_rs.contains("@executable_path/../Frameworks"),
            "build.rs must emit the @executable_path/../Frameworks rpath"
        );
    }

    #[test]
    fn the_release_workflow_generates_and_attaches_the_appcast() {
        let workflow = repo_file(".github/workflows/release.yml");
        let appcast = repo_file("scripts/make-appcast.sh");
        // Signing reads the private key from the repo secret via stdin — the
        // documented CI path (`--ed-key-file -`), never a command-line key.
        assert!(workflow.contains("secrets.SPARKLE_ED_KEY"));
        assert!(workflow.contains("--ed-key-file -"));
        assert!(
            !workflow.contains("generate_appcast") || workflow.contains("scripts/make-appcast.sh"),
            "the generate_appcast call lives in the script"
        );
        // The absent-secret path is a loud skip, not a failure: the appcast
        // script emits a workflow warning (shown as an annotation) and exits 0,
        // so the release still ships.
        assert!(
            appcast.contains("::warning::") && appcast.contains("SPARKLE_ED_KEY is not set"),
            "a missing secret must skip loudly (a workflow warning line + exit 0)"
        );
        assert!(
            appcast.contains("if [[ -z \"${SPARKLE_ED_KEY:-}\" ]]") && appcast.contains("exit 0"),
            "the skip must be the success path"
        );
        // The appcast is attached to the release.
        assert!(workflow.contains("appcast.xml"));
        assert!(
            appcast.contains("--download-url-prefix") && appcast.contains("releases/download/"),
            "the archive URLs must be the tag-specific GitHub release assets"
        );
        assert!(
            appcast.contains("--ed-key-file -")
                && appcast.contains("printf '%s\\n' \"$SPARKLE_ED_KEY\""),
            "the key is piped to stdin (the docs' secret pattern)"
        );
        assert!(
            appcast.contains(".md") && appcast.contains("CHANGELOG.md"),
            "release notes come from the CHANGELOG section as a same-named .md"
        );
    }

    #[test]
    fn the_ci_macos_job_compiles_the_updater_against_the_pinned_framework() {
        let workflow = repo_file(".github/workflows/ci.yml");
        assert!(
            workflow.contains("scripts/fetch-sparkle.sh"),
            "the macOS CI job fetches the pinned Sparkle distribution"
        );
        assert!(
            workflow.contains("SURAMAK_FEATURES") && workflow.contains("updates"),
            "the macOS CI bundle build must carry the `updates` feature"
        );
    }

    // ---- the appcast step's logic (runs on Linux with a stub tool) ----------

    /// The real `generate_appcast` is a macOS tool, but the script's *logic* —
    /// staging, the same-named `.md` release notes, the documented flags, the
    /// key on stdin, the absent-secret skip — is exercised here on Linux with
    /// a stub distribution whose `generate_appcast` records its argv and stdin
    /// and writes a plausible appcast.xml.
    #[cfg(unix)]
    #[test]
    fn make_appcast_stages_notes_pipes_the_key_and_skips_without_it() {
        use std::os::unix::fs::PermissionsExt as _;

        let root = repo_root();
        let scratch =
            std::env::temp_dir().join(format!("suramak-m12-appcast-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).unwrap();
        // Start from a clean staging dir (a previous run may have left one).
        let _ = std::fs::remove_dir_all(root.join("build/appcast"));

        // The stub distribution: bin/generate_appcast records argv + stdin.
        let stub = scratch.join("bin/generate_appcast");
        std::fs::create_dir_all(stub.parent().unwrap()).unwrap();
        std::fs::write(
            &stub,
            "#!/usr/bin/env bash\n\
             set -euo pipefail\n\
             out=\"$(pwd)/build/appcast\"\n\
             printf '%s\\n' \"$@\" > \"$out/.argv\"\n\
             cat > \"$out/.key\"\n\
             printf '<rss/>\\n' > \"$out/appcast.xml\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        // A stand-in for the packaged archive.
        let archive = scratch.join("Suramak-v0.2.0-macos-arm64.zip");
        std::fs::write(&archive, b"not a real zip - the stub never reads it").unwrap();

        let run = |secret: Option<&str>| {
            let mut command = std::process::Command::new("bash");
            command
                .arg("scripts/make-appcast.sh")
                .arg("v0.2.0")
                .arg(&archive)
                .current_dir(&root)
                .env("SPARKLE_DIST_DIR", &scratch)
                .env_remove("GITHUB_ACTIONS")
                .env_remove("SURAMAK_APPCAST_HISTORY");
            match secret {
                Some(secret) => command.env("SPARKLE_ED_KEY", secret),
                None => command.env_remove("SPARKLE_ED_KEY"),
            };
            command.output().expect("run scripts/make-appcast.sh")
        };

        // 1. Absent secret: a loud workflow warning, exit 0, nothing staged.
        let skipped = run(None);
        let stdout = String::from_utf8_lossy(&skipped.stdout);
        assert!(
            skipped.status.success(),
            "the absent-secret path must succeed: {stdout}"
        );
        assert!(
            stdout.contains("::warning::") && stdout.contains("SPARKLE_ED_KEY is not set"),
            "the absent-secret path must log loudly: {stdout}"
        );
        assert!(
            !root.join("build/appcast/appcast.xml").exists(),
            "no appcast without a key"
        );

        // 2. With a key (the 44-char current format): staged, signed via stdin.
        let key = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
        assert_eq!(key.len(), 44, "the stub key must have the documented shape");
        let run_ok = run(Some(key));
        let stdout = String::from_utf8_lossy(&run_ok.stdout);
        let stderr = String::from_utf8_lossy(&run_ok.stderr);
        assert!(
            run_ok.status.success(),
            "the signed path must succeed: {stdout}\n{stderr}"
        );
        let stage = root.join("build/appcast");
        let argv = std::fs::read_to_string(stage.join(".argv")).expect("the stub recorded argv");
        let args: Vec<&str> = argv.lines().collect();
        assert!(
            args.contains(&"--ed-key-file") && args.contains(&"-"),
            "the key must arrive via `--ed-key-file -`: {argv}"
        );
        assert!(
            args.contains(&"--download-url-prefix"),
            "the archive URLs need the download prefix: {argv}"
        );
        assert!(
            args.contains(&"https://github.com/LoLyeah/suramak/releases/download/v0.2.0/"),
            "the prefix must be this tag's release assets: {argv}"
        );
        assert!(
            args.contains(&"--embed-release-notes"),
            "the notes are embedded into the appcast (no extra asset to host): {argv}"
        );
        assert_eq!(
            std::fs::read_to_string(stage.join(".key")).unwrap().trim(),
            key,
            "the secret must reach the tool on stdin, byte for byte"
        );
        // The same-named .md notes file, from the CHANGELOG section.
        let notes = stage.join("Suramak-v0.2.0-macos-arm64.md");
        let notes_text = std::fs::read_to_string(&notes).expect("the notes file must exist");
        assert!(
            notes_text.contains("Native Opus import"),
            "the notes must be the CHANGELOG section for the tag: {}",
            &notes_text[..notes_text.len().min(200)]
        );
        // …and an unknown tag falls back to the pointer line, never an empty file.
        std::fs::write(&archive, b"x").unwrap();
        let fallback_archive = scratch.join("Suramak-v9.9.9-macos-arm64.zip");
        std::fs::write(&fallback_archive, b"x").unwrap();
        let fallback = std::process::Command::new("bash")
            .arg("scripts/make-appcast.sh")
            .arg("v9.9.9")
            .arg(&fallback_archive)
            .current_dir(&root)
            .env("SPARKLE_DIST_DIR", &scratch)
            .env("SPARKLE_ED_KEY", key)
            .env_remove("GITHUB_ACTIONS")
            .env_remove("SURAMAK_APPCAST_HISTORY")
            .output()
            .expect("run the fallback case");
        assert!(fallback.status.success());
        assert_eq!(
            std::fs::read_to_string(stage.join("Suramak-v9.9.9-macos-arm64.md")).unwrap(),
            "See CHANGELOG.md.\n"
        );

        // 3. The double-encoded-secret mistake is refused with the right hint.
        let double_encoded = "QUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQUFBQT0=";
        assert_eq!(double_encoded.len(), 60, "base64 of the 44-char key");
        let bad = run(Some(double_encoded));
        let stderr = String::from_utf8_lossy(&bad.stderr);
        assert!(
            !bad.status.success(),
            "a wrong-shaped secret must fail loudly"
        );
        assert!(
            stderr.contains("generate_keys -x"),
            "the refusal must explain the expected secret: {stderr}"
        );

        let _ = std::fs::remove_dir_all(&scratch);
    }

    // ---- the stub's shape ---------------------------------------------------

    #[test]
    fn the_stub_matches_the_platform() {
        assert_eq!(
            compiled(),
            cfg!(all(feature = "updates", target_os = "macos"))
        );
        if !compiled() {
            // Off-macOS / feature-off: start and the policy setter are no-ops.
            let mut updater = Updater::start(true);
            updater.set_automatic_checks(false);
            updater.set_automatic_checks(true);
        }
    }
}
