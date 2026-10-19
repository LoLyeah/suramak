//! Build script: two platform-only jobs, each a strict no-op on every other
//! target.
//!
//! ## macOS + `updates` (M12)
//!
//! The `updates` feature links `Sparkle.framework` (through `sparkle-updater`'s
//! build script) and the packaged app embeds the framework in
//! `Contents/Frameworks` — so the executable needs a runtime search path that
//! resolves inside the bundle: `@executable_path/../Frameworks` (the rpath
//! Sparkle's setup guide and the crate's README both name).
//!
//! Emitted **only** for macOS builds with the feature on; every other build
//! (all Linux/Windows builds, and macOS builds without `updates`) emits nothing
//! and links exactly as before.
//!
//! This script does not touch the framework itself: `scripts/fetch-sparkle.sh`
//! pins + verifies it, `scripts/make-app.sh` embeds + signs it, and
//! `sparkle-updater`'s own build script links it (it needs
//! `SPARKLE_FRAMEWORK_PATH` — the fetch script prints exactly that directory).
//!
//! ## Windows (M14b)
//!
//! The exe's icon, VERSIONINFO and **application manifest** (PerMonitorV2 DPI
//! awareness, `longPathAware`, UTF-8 `activeCodePage`, Common Controls v6, the
//! Win10/11 `supportedOS` GUID) are committed as resource files under
//! `packaging/windows/` — the analogue of `packaging/Info.plist` — and compiled
//! into **the binary only** (`cargo:rustc-link-arg-bins`), which is why this
//! crate has a `[[bin]]`.
//!
//! Off Windows the resource half checks the `.rc`'s version literals against
//! `CARGO_PKG_VERSION` (so drift fails *every* target's build) and returns before
//! touching `embed-resource`: a Linux or macOS build runs this script and embeds
//! nothing (provable with `cargo build -p suramak-ui -vv`: the build-script line
//! and its three `rerun-if-changed` lines appear, no `rc.exe`/`windres` invocation
//! follows, and no `.res`/`.lib` lands in `target/*/build/suramak-ui-*/out`).
//!
//! A failure to compile the resources is a build error, never a silent skip:
//! shipping without the manifest would lose the DPI/UTF-8/long-path declarations
//! — the point of the slice — and the exe's properties would lose the version.

fn main() {
    macos_sparkle_rpath();
    windows_resources();
}

/// M12: `@executable_path/../Frameworks` on macOS builds with `updates`.
fn macos_sparkle_rpath() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    if std::env::var_os("CARGO_FEATURE_UPDATES").is_none() {
        return;
    }
    println!("cargo:rustc-link-arg-bins=-Wl,-rpath,@executable_path/../Frameworks");
}

/// M14b: the Windows resources (icon + VERSIONINFO + application manifest).
fn windows_resources() {
    let crate_dir =
        std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let packaging = crate_dir.join("../../packaging/windows");
    let rc = packaging.join("suramak.rc");

    for name in ["suramak.rc", "suramak.manifest", "suramak.ico"] {
        println!("cargo:rerun-if-changed={}", packaging.join(name).display());
    }

    // The `.rc` carries the version as literals (RC's preprocessor cannot read
    // Cargo's), so fail the build the moment the two drift apart instead of
    // shipping an exe whose properties disagree with `CARGO_PKG_VERSION`. Checked
    // on **every** target — a version bump that forgets the resource file fails
    // the Linux and macOS jobs too, not only the Windows one.
    let version = std::env::var("CARGO_PKG_VERSION").expect("cargo sets it");
    check_version_matches(&rc, &version);

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .unwrap_or_else(|err| panic!("M14b: cannot embed the Windows resources: {err}"));
}

/// Asserts that `suramak.rc` states the same version as Cargo.
///
/// Checked: `FILEVERSION a,b,c,0` (the numeric form Windows compares) and the
/// `"FileVersion"` / `"ProductVersion"` strings. A mismatch names the lines to
/// edit.
fn check_version_matches(rc: &std::path::Path, version: &str) {
    let text = std::fs::read_to_string(rc)
        .unwrap_or_else(|err| panic!("M14b: cannot read {}: {err}", rc.display()));
    // RC ignores runs of whitespace and the file aligns its columns, so compare a
    // normalised copy — otherwise the check would be about formatting.
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut parts = version.split('.');
    let numeric = |part: Option<&str>| -> u64 {
        part.and_then(|p| p.parse::<u64>().ok())
            .unwrap_or_else(|| panic!("M14b: CARGO_PKG_VERSION {version:?} is not a.b.c"))
    };
    let (major, minor, patch) = (
        numeric(parts.next()),
        numeric(parts.next()),
        numeric(parts.next()),
    );
    let expected = [
        format!("FILEVERSION {major},{minor},{patch},0"),
        format!("VALUE \"FileVersion\", \"{version}\""),
        format!("VALUE \"ProductVersion\", \"{version}\""),
    ];
    for needle in expected {
        assert!(
            text.contains(&needle),
            "M14b: packaging/windows/suramak.rc is out of date — expected {needle:?} \
             (CARGO_PKG_VERSION = {version}). Update the VERSIONINFO block."
        );
    }
}
