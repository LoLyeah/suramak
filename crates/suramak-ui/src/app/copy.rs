//! Platform-resolved copy (M14e) — the **one** place a sentence that names the
//! operating system is written.
//!
//! The owner's Windows finding: Settings said *"System follows the macOS
//! light/dark setting live"* on Windows. The cause was copy written once for
//! the Mac and never resolved per platform. The fix is a resolver, not a second
//! sentence in a second branch: every platform-aware caption is a function of
//! [`Platform`], and [`Platform::current`] is the only thing that reads the
//! compile target. Tests can therefore check **all three** variants from any
//! host (a `cfg!`-only branch would leave the Windows sentence untested on CI,
//! which is exactly how the wrong word shipped).

/// The desktop platforms this app ships on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Windows,
    Linux,
}

impl Platform {
    /// The platform this build runs on.
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// The platform's name as user-facing copy ("macOS", "Windows", "Linux").
    // Used by the copy tests, which exercise all three variants from any host.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const fn name(self) -> &'static str {
        match self {
            Platform::MacOs => "macOS",
            Platform::Windows => "Windows",
            Platform::Linux => "Linux",
        }
    }
}

/// What "System" appearance means on each platform — the caption under the
/// Theme row.
///
/// macOS and Windows both report a light/dark preference to the app and follow
/// it live (winit's `Window::theme`); Linux desktops only sometimes report one,
/// and `theme::effective_theme` pins an unreported System to Light, so the
/// Linux sentence says exactly that instead of promising a live follow.
pub fn theme_caption(platform: Platform) -> &'static str {
    match platform {
        Platform::MacOs => {
            "System follows the macOS light/dark setting live; Light or Dark forces this app's \
             appearance. Applied when you press Save."
        }
        Platform::Windows => {
            "System follows the Windows light/dark setting live; Light or Dark forces this app's \
             appearance. Applied when you press Save."
        }
        Platform::Linux => {
            "System follows the desktop theme when the desktop reports one (Linux falls back to \
             Light otherwise); Light or Dark forces this app's appearance. Applied when you \
             press Save."
        }
    }
}

/// [`theme_caption`] for this build.
pub fn theme_caption_here() -> &'static str {
    theme_caption(Platform::current())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Platform; 3] = [Platform::MacOs, Platform::Windows, Platform::Linux];

    #[test]
    fn every_platform_gets_its_own_theme_sentence() {
        for platform in ALL {
            let caption = theme_caption(platform);
            assert!(
                caption.contains(platform.name()),
                "{platform:?}: the caption must name this platform, got {caption:?}"
            );
            // …and must never name a different OS (the Windows bug: a macOS
            // sentence rendered on Windows hardware).
            for other in ALL.iter().filter(|other| **other != platform) {
                assert!(
                    !caption.contains(other.name()),
                    "{platform:?}: the caption names {} — cross-platform copy leaked: {caption:?}",
                    other.name()
                );
            }
            // The rest of the sentence is shared (one resolver, not three
            // hand-written copies that can drift).
            assert!(caption.contains("forces this app's appearance"));
            assert!(caption.contains("Applied when you press Save."));
        }
    }

    #[test]
    fn the_linux_caption_promises_only_what_the_resolver_does() {
        let caption = theme_caption(Platform::Linux);
        assert!(
            caption.contains("when the desktop reports one") && caption.contains("falls back to"),
            "Linux has no guaranteed live follow — the caption must say so: {caption:?}"
        );
    }

    #[test]
    fn this_build_resolves_to_its_own_platform() {
        let here = Platform::current();
        assert_eq!(theme_caption_here(), theme_caption(here));
        assert_eq!(
            here == Platform::MacOs,
            cfg!(target_os = "macos"),
            "the resolver must follow the compile target"
        );
        assert_eq!(here == Platform::Windows, cfg!(target_os = "windows"));
        assert_eq!(
            here == Platform::Linux,
            cfg!(not(any(target_os = "macos", target_os = "windows")))
        );
    }
}
