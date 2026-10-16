//! M12: the `updates` feature links `Sparkle.framework` (through
//! `sparkle-updater`'s build script) and the packaged app embeds the framework
//! in `Contents/Frameworks` — so the executable needs a runtime search path
//! that resolves inside the bundle: `@executable_path/../Frameworks` (the
//! rpath Sparkle's setup guide and the crate's README both name).
//!
//! Emitted **only** for macOS builds with the feature on; every other build
//! (all Linux/Windows builds, and macOS builds without `updates`) emits
//! nothing and links exactly as before.
//!
//! This script does not touch the framework itself: `scripts/fetch-sparkle.sh`
//! pins + verifies it, `scripts/make-app.sh` embeds + signs it, and
//! `sparkle-updater`'s own build script links it (it needs
//! `SPARKLE_FRAMEWORK_PATH` — the fetch script prints exactly that directory).

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    if std::env::var_os("CARGO_FEATURE_UPDATES").is_none() {
        return;
    }
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/../Frameworks");
}
