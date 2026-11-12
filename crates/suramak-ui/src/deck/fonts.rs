//! The bundled OFL faces (M20A) — the Deck's typography, embedded.
//!
//! Until M20A the Deck shipped no font files at all: `theme::install_fonts`
//! loaded SF Pro / New York / SF Mono from `/System/Library/Fonts` on macOS
//! and silently did nothing everywhere else, so Android — and the Linux
//! goldens — rendered egui's baked-in Ubuntu-Light/Hack. That is the owner's
//! "especially the font".
//!
//! epaint 0.33 rasterises with `ab_glyph`: it cannot interpolate a variable
//! font's `wght` axis and cannot toggle OpenType features. So the five faces
//! here are **static instances**, subsetted offline by
//! `scripts/fonts/build_fonts.py` (which also writes `PROVENANCE.md`), and
//! committed under `assets/fonts/`. They are compiled into the binary with
//! `include_bytes!` — no asset directory to find at runtime, nothing to copy
//! into the APK beyond the `.so` that already contains them.
//!
//! macOS keeps its system faces (SF Pro / New York / SF Mono) unchanged: the
//! bundled table is empty there, so the bytes are not even linked into a macOS
//! binary (`#[cfg]` on the `include_bytes!`, not a runtime check).
//!
//! The type scale that consumes these faces is [`crate::deck::type_scale`];
//! the registration itself is `theme::install_fonts`.

/// Inter Regular 400 — `Proportional`: body, row values, captions.
pub const INTER_REGULAR: &str = "suramak-inter-regular";
/// Inter Medium 500 — `SansMedium`: row titles, numerals.
pub const INTER_MEDIUM: &str = "suramak-inter-medium";
/// Inter SemiBold 600 — `SansSemiBold`: screen titles, section labels.
pub const INTER_SEMIBOLD: &str = "suramak-inter-semibold";
/// Newsreader Regular 400 — the serif voice (`serif_family()`).
pub const NEWSREADER_REGULAR: &str = "suramak-newsreader";
/// Geist Mono Regular 400 — `Monospace`: diagnostics, hashes, paths.
pub const GEIST_MONO_REGULAR: &str = "suramak-geist-mono";

/// One embedded face: the egui `font_data` key, the committed file it came
/// from (the provenance key), the upstream weight, and the bytes.
#[derive(Debug, Clone, Copy)]
pub struct BundledFace {
    pub id: &'static str,
    pub file: &'static str,
    pub weight: u16,
    pub bytes: &'static [u8],
}

/// The five faces, in registration order.
#[cfg(not(target_os = "macos"))]
pub const BUNDLED: [BundledFace; 5] = [
    BundledFace {
        id: INTER_REGULAR,
        file: "Inter-Regular.ttf",
        weight: 400,
        bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/Inter-Regular.ttf"
        )),
    },
    BundledFace {
        id: INTER_MEDIUM,
        file: "Inter-Medium.ttf",
        weight: 500,
        bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/Inter-Medium.ttf"
        )),
    },
    BundledFace {
        id: INTER_SEMIBOLD,
        file: "Inter-SemiBold.ttf",
        weight: 600,
        bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/Inter-SemiBold.ttf"
        )),
    },
    BundledFace {
        id: NEWSREADER_REGULAR,
        file: "Newsreader-Regular.ttf",
        weight: 400,
        bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/Newsreader-Regular.ttf"
        )),
    },
    BundledFace {
        id: GEIST_MONO_REGULAR,
        file: "GeistMono-Regular.ttf",
        weight: 400,
        bytes: include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../assets/fonts/GeistMono-Regular.ttf"
        )),
    },
];

/// macOS embeds nothing: its faces are the system's, loaded at runtime.
#[cfg(target_os = "macos")]
pub const BUNDLED: [BundledFace; 0] = [];

/// One macOS system face: what theme's macOS loader reads, unchanged since
/// M9a. The table lives here (and is `pub`) so the platform rule is testable
/// on any host — the loading itself stays macOS-gated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemFace {
    pub id: &'static str,
    pub path: &'static str,
    /// Which Deck role the face fills.
    pub role: SystemRole,
}

/// The roles the Deck asks for by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemRole {
    Proportional,
    Serif,
    Monospace,
}

/// SF Pro — the macOS proportional face.
pub const SF_PRO: &str = "suramak-sf-pro";
/// New York — the macOS serif face.
pub const NEW_YORK: &str = "suramak-new-york";
/// SF Mono — the macOS mono face.
pub const SF_MONO: &str = "suramak-sf-mono";

/// The macOS system table. Paths and ids are the M9a values, pinned here so a
/// refactor cannot silently point macOS at a different file: the workorder's
/// rule is "macOS keeps SF Pro / New York / SF Mono (unchanged behaviour)".
pub const MACOS_SYSTEM: [SystemFace; 3] = [
    SystemFace {
        id: SF_PRO,
        path: "/System/Library/Fonts/SFNS.ttf",
        role: SystemRole::Proportional,
    },
    SystemFace {
        id: NEW_YORK,
        path: "/System/Library/Fonts/NewYork.ttf",
        role: SystemRole::Serif,
    },
    SystemFace {
        id: SF_MONO,
        path: "/System/Library/Fonts/SFNSMono.ttf",
        role: SystemRole::Monospace,
    },
];

/// Which table this build registers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// macOS: the system faces, loaded from `/System/Library/Fonts`.
    System,
    /// Android / Windows / Linux: the five embedded statics.
    Bundled,
}

/// The platform this build registers (compile-time, not a runtime probe).
pub const fn platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::System
    } else {
        Platform::Bundled
    }
}

/// The bundled faces for this build (empty on macOS).
pub fn bundled_faces() -> &'static [BundledFace] {
    &BUNDLED
}

/// The macOS system table — always available, so a Linux test can pin the
/// macOS rule without a macOS runner.
pub fn macos_system_faces() -> &'static [SystemFace] {
    &MACOS_SYSTEM
}

/// Looks a bundled face up by its egui `font_data` key.
pub fn bundled_face(id: &str) -> Option<&'static BundledFace> {
    bundled_faces().iter().find(|face| face.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sfnt magic numbers egui's rasteriser accepts (the same sniff
    /// `theme::looks_like_font` does on macOS).
    fn is_sfnt(bytes: &[u8]) -> bool {
        matches!(
            bytes.get(..4),
            Some(
                [0x00, 0x01, 0x00, 0x00]
                    | [b'O', b'T', b'T', b'O']
                    | [b't', b'r', b'u', b'e']
                    | [b't', b't', b'c', b'f']
            )
        )
    }

    /// Off macOS the table is exactly the five committed statics, each inside
    /// the workorder's 120 KB budget, each a real sfnt, all distinct.
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_bundled_table_is_the_five_committed_faces() {
        let faces = bundled_faces();
        assert_eq!(faces.len(), 5);
        assert_eq!(
            faces.iter().map(|face| face.id).collect::<Vec<_>>(),
            vec![
                INTER_REGULAR,
                INTER_MEDIUM,
                INTER_SEMIBOLD,
                NEWSREADER_REGULAR,
                GEIST_MONO_REGULAR
            ]
        );
        assert_eq!(
            faces.iter().map(|face| face.weight).collect::<Vec<_>>(),
            vec![400, 500, 600, 400, 400]
        );
        for face in faces {
            assert!(
                face.bytes.len() > 4_000,
                "{}: {} bytes is too small to be a font",
                face.file,
                face.bytes.len()
            );
            assert!(
                face.bytes.len() <= 120 * 1024,
                "{}: {} bytes is over the 120 KB ceiling",
                face.file,
                face.bytes.len()
            );
            assert!(is_sfnt(face.bytes), "{}: not an sfnt", face.file);
            assert!(
                face.file.ends_with(".ttf"),
                "{}: the subset is a TTF",
                face.file
            );
        }
        // No two faces are the same bytes (a copy-paste in the table would
        // silently render every weight as the same file).
        for (index, left) in faces.iter().enumerate() {
            for right in &faces[index + 1..] {
                assert_ne!(left.bytes, right.bytes, "{} == {}", left.file, right.file);
            }
        }
        // The total weight the phone's .so carries.
        let total: usize = faces.iter().map(|face| face.bytes.len()).sum();
        assert!(
            total <= 5 * 120 * 1024,
            "the five faces must stay inside five budgets: {total}"
        );
        assert_eq!(
            bundled_face(INTER_MEDIUM).map(|face| face.weight),
            Some(500)
        );
        assert!(bundled_face("not-a-face").is_none());
    }

    /// The macOS table is the M9a one: same ids, same three system paths, same
    /// roles. The workorder's rule is "macOS keeps SF Pro / New York / SF Mono
    /// (unchanged behaviour)", and this pins it on every host.
    #[test]
    fn the_macos_table_is_the_unchanged_system_faces() {
        assert_eq!(
            macos_system_faces(),
            &[
                SystemFace {
                    id: SF_PRO,
                    path: "/System/Library/Fonts/SFNS.ttf",
                    role: SystemRole::Proportional,
                },
                SystemFace {
                    id: NEW_YORK,
                    path: "/System/Library/Fonts/NewYork.ttf",
                    role: SystemRole::Serif,
                },
                SystemFace {
                    id: SF_MONO,
                    path: "/System/Library/Fonts/SFNSMono.ttf",
                    role: SystemRole::Monospace,
                },
            ]
        );
        // Exactly one table is live, decided at compile time.
        assert_eq!(platform() == Platform::System, cfg!(target_os = "macos"));
        #[cfg(target_os = "macos")]
        assert!(bundled_faces().is_empty(), "macOS links no bundled bytes");
    }
}
