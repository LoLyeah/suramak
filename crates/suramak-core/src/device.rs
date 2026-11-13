//! Device-appropriate model guidance for phones (M13d).
//!
//! The desktop model manager shows every pinned model; a phone cannot run
//! them all. This module is the single source of truth for the Android
//! landing screen and the download refusal path:
//!
//! * the **supported floor** is a 64-bit ARM device with 4 GB of RAM
//!   (owner decision, 2026-10-16);
//! * below the floor only `tiny` runs, and every larger model is refused with
//!   a message that names the requirement;
//! * 4–6 GB devices default to `base`, 6 GB devices may choose `small`, and
//!   8 GB and up default to `small`;
//! * Indonesian is never steered to `tiny` — its Indonesian accuracy is poor.
//!
//! Android reports *usable* RAM (`ActivityManager.MemoryInfo.totalMem`), which
//! is below the nominal figure printed on the box: a "4 GB" phone reports
//! roughly 3.6 GB. The thresholds below are usable-RAM values with that gap
//! built in, so a 4 GB device lands in [`PhoneTier::Low`], not
//! [`PhoneTier::BelowFloor`], and a 3 GB device does not sneak in.
//!
//! The per-model rules live on the manifest ([`crate::models::PhoneGuidance`]),
//! not here: the recommendation is derived from [`crate::models::MODELS`], so
//! a pin and its phone guidance cannot drift apart.

use crate::limits::{self, MemoryProbe, Platform};
use crate::models::{ModelInfo, MODELS};

const MIB: u64 = 1024 * 1024;

/// Usable-RAM floor for a nominal 4 GB phone (~3.6 GB reported).
pub const FLOOR_RAM_BYTES: u64 = 3_400 * MIB;
/// Usable-RAM boundary for a nominal 6 GB phone (~5.5 GB reported).
pub const MID_RAM_BYTES: u64 = 5_000 * MIB;
/// Usable-RAM boundary for a nominal 8 GB phone (~7.4 GB reported).
pub const HIGH_RAM_BYTES: u64 = 6_800 * MIB;

/// What the platform reported about the device running the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DeviceProfile {
    /// `ActivityManager.MemoryInfo.totalMem` — usable RAM, not nominal.
    pub total_ram_bytes: u64,
    /// `ActivityManager.MemoryInfo.availMem` at probe time.
    pub available_ram_bytes: u64,
    /// `ActivityManager.isLowRamDevice` — the OS's own "this is a small
    /// device" flag. It wins over the byte count: a 4 GB phone that the
    /// platform marks low-RAM is treated as below the floor.
    pub low_memory: bool,
    /// Whether the process runs on arm64 (`Build.SUPPORTED_64_BIT_ABIS`).
    pub arm64: bool,
}

impl DeviceProfile {
    pub fn new(
        total_ram_bytes: u64,
        available_ram_bytes: u64,
        low_memory: bool,
        arm64: bool,
    ) -> Self {
        Self {
            total_ram_bytes,
            available_ram_bytes,
            low_memory,
            arm64,
        }
    }

    /// Total usable RAM in decimal GB — the number the guidance quotes.
    pub fn ram_gb(self) -> f64 {
        self.total_ram_bytes as f64 / 1_000_000_000.0
    }
}

/// The RAM class a phone falls into. `Unsupported` is about the CPU, the rest
/// are about memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhoneTier {
    /// Not a 64-bit ARM device: no model can run.
    Unsupported,
    /// arm64, but below the 4 GB floor: only `tiny` runs.
    BelowFloor,
    /// A nominal 4 GB phone: `base` is the default.
    Low,
    /// A nominal 6 GB phone: `base` is the default, `small` is available.
    Mid,
    /// A nominal 8 GB (or larger) phone: `small` is the default.
    High,
}

impl PhoneTier {
    /// Short human label for the landing screen.
    pub fn label(self) -> &'static str {
        match self {
            PhoneTier::Unsupported => "unsupported CPU",
            PhoneTier::BelowFloor => "below the 4 GB floor",
            PhoneTier::Low => "4 GB",
            PhoneTier::Mid => "6 GB",
            PhoneTier::High => "8 GB or more",
        }
    }

    /// Whether the device meets the supported floor (4 GB arm64).
    pub fn meets_floor(self) -> bool {
        matches!(self, PhoneTier::Low | PhoneTier::Mid | PhoneTier::High)
    }
}

/// Classifies a device from its memory report.
pub fn tier_for(profile: DeviceProfile) -> PhoneTier {
    if !profile.arm64 {
        return PhoneTier::Unsupported;
    }
    if profile.low_memory || profile.total_ram_bytes < FLOOR_RAM_BYTES {
        return PhoneTier::BelowFloor;
    }
    if profile.total_ram_bytes < MID_RAM_BYTES {
        return PhoneTier::Low;
    }
    if profile.total_ram_bytes < HIGH_RAM_BYTES {
        return PhoneTier::Mid;
    }
    PhoneTier::High
}

/// One model the device cannot run, with the message the UI shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRefusal {
    pub id: &'static str,
    pub reason: String,
}

/// What the landing screen and the download path need for one device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recommendation {
    pub tier: PhoneTier,
    /// The model to preselect. `tiny` below the floor (the only option),
    /// `base` on 4–6 GB, `small` at 8 GB and up. `None` only when the CPU is
    /// unsupported (nothing can run).
    pub default_model_id: Option<&'static str>,
    /// Manifest ids this device may download, cheapest first.
    pub allowed: Vec<&'static str>,
    /// Manifest ids refused here, each with its reason.
    pub refused: Vec<ModelRefusal>,
    /// One short line for the top of the screen.
    pub headline: &'static str,
    /// The honest detail sentence (RAM, default, what needs more).
    pub detail: String,
    /// Language-specific steering; only set for Indonesian today.
    pub language_note: Option<String>,
}

/// Whether a language tag is Indonesian (`id`, `id-ID`, `ID_id` …).
pub fn is_indonesian(language: &str) -> bool {
    language
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .eq_ignore_ascii_case("id")
}

/// The guidance for `model` on `profile`, or `None` when it may run.
pub fn refusal_for(profile: DeviceProfile, _language: &str, model_id: &str) -> Option<String> {
    let tier = tier_for(profile);
    let model = MODELS.iter().find(|model| model.id == model_id)?;
    if tier == PhoneTier::Unsupported {
        return Some(
            "Suramak needs a 64-bit ARM phone; this device's CPU is not supported.".to_string(),
        );
    }
    if model.phone.min_ram_bytes <= profile.total_ram_bytes {
        // M26A: the tier says the model is offered here; the device's own
        // budget says whether it can actually load. The number is computed by
        // the same formula the queue and the recorder use.
        return budget_refusal(profile, model_id);
    }
    Some(refusal_reason(model, profile, tier))
}

/// M26A — the model-fit gate for a phone.
///
/// `None` when the model fits (or is outside the measured model table, which
/// this build never gates by a number it did not measure). When it does not
/// fit, the sentence is [`limits::model_fit_refusal`]: the model's need, the
/// device's budget, and the largest models that do fit. There is no silent
/// downgrade in this path — the model is refused and the user chooses.
pub fn budget_refusal(profile: DeviceProfile, model_id: &str) -> Option<String> {
    let footprint = limits::footprint_for(model_id)?;
    // The gate is about whether the model can **ever** load on this device, so
    // it uses the device's installed memory, not this instant's free pages
    // (availMem moves by the second and would make the picker flicker).
    let probe = MemoryProbe::new(profile.total_ram_bytes, None, profile.low_memory);
    let limits = limits::compute_for(Platform::Android, probe, footprint, None);
    limits::model_fit_refusal(footprint, &limits)
}

/// The refusal sentence for one model on one device.
fn refusal_reason(model: &ModelInfo, profile: DeviceProfile, tier: PhoneTier) -> String {
    if model.phone.min_ram_bytes == u64::MAX {
        return model.phone.note.to_string();
    }
    if tier == PhoneTier::BelowFloor {
        return format!(
            "{} needs a phone with at least 4 GB of RAM; this one has {:.1} GB",
            model.id,
            profile.ram_gb()
        );
    }
    let needed = if model.phone.min_ram_bytes >= HIGH_RAM_BYTES {
        "8 GB"
    } else if model.phone.min_ram_bytes >= MID_RAM_BYTES {
        "6 GB"
    } else {
        "4 GB"
    };
    format!(
        "{} needs a phone with at least {needed} of RAM; this one has {:.1} GB",
        model.id,
        profile.ram_gb()
    )
}

/// The full recommendation for a device and UI language.
pub fn recommend(profile: DeviceProfile, language: &str) -> Recommendation {
    let tier = tier_for(profile);
    let indonesian = is_indonesian(language);

    let mut allowed = Vec::new();
    let mut refused = Vec::new();
    for model in MODELS {
        if tier == PhoneTier::Unsupported {
            refused.push(ModelRefusal {
                id: model.id,
                reason: "Suramak needs a 64-bit ARM phone; this device's CPU is not supported."
                    .to_string(),
            });
        } else if model.phone.min_ram_bytes <= profile.total_ram_bytes {
            // M26A: the tier offers it; the device budget must also fit it.
            match budget_refusal(profile, model.id) {
                None => allowed.push(model.id),
                Some(reason) => refused.push(ModelRefusal {
                    id: model.id,
                    reason,
                }),
            }
        } else {
            refused.push(ModelRefusal {
                id: model.id,
                reason: refusal_reason(model, profile, tier),
            });
        }
    }

    // The default is the model with the highest "default from" threshold the
    // device clears. `tiny` has 0 and is therefore only the fallback when it
    // is the sole allowed model (below the floor).
    let mut default_model_id = MODELS
        .iter()
        .filter(|model| allowed.contains(&model.id))
        .filter(|model| {
            model.phone.default_from_ram_bytes > 0
                && model.phone.default_from_ram_bytes <= profile.total_ram_bytes
        })
        .max_by_key(|model| model.phone.default_from_ram_bytes)
        .map(|model| model.id);
    if default_model_id.is_none() && allowed.len() == 1 {
        default_model_id = allowed.first().copied();
    }
    // A tier default the budget refuses (a memory-starved device) is replaced
    // by the largest model that does fit — a preselection, never the user's
    // explicit choice.
    if default_model_id.is_none() {
        default_model_id = MODELS
            .iter()
            .filter(|model| allowed.contains(&model.id))
            .max_by_key(|model| model.phone.min_ram_bytes)
            .map(|model| model.id);
    }

    let headline = match tier {
        PhoneTier::Unsupported => "This phone is not supported",
        PhoneTier::BelowFloor => "Below the 4 GB floor — only tiny runs",
        PhoneTier::Low | PhoneTier::Mid => "base is the default on this phone",
        PhoneTier::High => "small is the default on this phone",
    };

    let detail = match tier {
        PhoneTier::Unsupported => "Suramak needs a 64-bit ARM (arm64) phone. This device reports a different CPU type, so no model can run.".to_string(),
        PhoneTier::BelowFloor => format!(
            "This phone reports {:.1} GB of RAM, below the supported 4 GB floor. Only tiny ({} MB) runs; base, small and the turbo models are refused here.",
            profile.ram_gb(),
            size_mb(model_by_id("tiny")),
        ),
        PhoneTier::Low => format!(
            "base ({} MB) is the on-device default. tiny is for quick tests only; small needs a 6 GB phone.",
            size_mb(model_by_id("base")),
        ),
        PhoneTier::Mid => format!(
            "base ({} MB) is the default; small ({} MB) is available when you want better accuracy.",
            size_mb(model_by_id("base")),
            size_mb(model_by_id("small")),
        ),
        PhoneTier::High => format!(
            "small ({} MB) is the default. base is lighter; turbo-q5-0 is slow but higher quality.",
            size_mb(model_by_id("small")),
        ),
    };

    let language_note = if !indonesian {
        None
    } else if default_model_id == Some("tiny") {
        Some(
            "Indonesian: tiny's Indonesian accuracy is poor, but this phone is below the 4 GB floor — no larger model can run here."
                .to_string(),
        )
    } else {
        Some(
            "Indonesian: use base or small — tiny's Indonesian accuracy is noticeably worse. Do not choose tiny for Indonesian."
                .to_string(),
        )
    };

    Recommendation {
        tier,
        default_model_id,
        allowed,
        refused,
        headline,
        detail,
        language_note,
    }
}

fn model_by_id(id: &str) -> &'static ModelInfo {
    MODELS
        .iter()
        .find(|model| model.id == id)
        .expect("device guidance names a manifest model")
}

fn size_mb(model: &ModelInfo) -> u64 {
    model.size_bytes / 1_000_000
}

/// The best model that is **verified on disk** and may run here (M19B).
///
/// "Highest tier" is the manifest's own `min_ram_bytes` ladder (tiny → base →
/// small → turbo-q5-0), with the pinned size as a tie-break, restricted to the
/// models this device is allowed to run. `None` when nothing usable is
/// installed.
pub fn highest_tier_verified(profile: DeviceProfile, verified: &[&str]) -> Option<&'static str> {
    if tier_for(profile) == PhoneTier::Unsupported {
        return None;
    }
    MODELS
        .iter()
        .filter(|model| verified.contains(&model.id))
        .filter(|model| model.phone.min_ram_bytes <= profile.total_ram_bytes)
        .max_by_key(|model| (model.phone.min_ram_bytes, model.size_bytes))
        .map(|model| model.id)
}

/// M19B: the model the Android host should store for this phone at load.
///
/// `None` means "keep what is stored". A change is proposed only when **all**
/// of these hold:
///
/// * the user never chose the model (`model_explicitly_chosen == false`);
/// * the stored model is the untouched desktop default (`turbo-q5-0`);
/// * that model is not verified on disk.
///
/// The replacement is the highest-tier *verified* model this phone may run, or
/// — when nothing usable is installed — the device tier's default (`small` at
/// 8 GB+, `base` at 4–6 GB, `tiny` below the floor). A user's explicit choice
/// is never overridden, and a verified desktop default is kept: it is a real
/// file the user can use.
pub fn model_choice_for_phone(
    model_id: &str,
    explicitly_chosen: bool,
    profile: DeviceProfile,
    verified: &[&str],
) -> Option<&'static str> {
    if explicitly_chosen || model_id != crate::models::DESKTOP_DEFAULT_MODEL_ID {
        return None;
    }
    if verified.contains(&crate::models::DESKTOP_DEFAULT_MODEL_ID) {
        return None;
    }
    let best = highest_tier_verified(profile, verified)
        .or_else(|| recommend(profile, "en").default_model_id)?;
    (best != model_id).then_some(best)
}

/// The Android landing copy, in the app's UI language.
///
/// Prepared here (not in the UI crate) so the text ships with the same rules
/// that enforce it: a landing screen that says "base or small" cannot drift
/// from a recommendation that steers Indonesian away from `tiny`.
pub const LANDING_EN: &str = "Choose a model for this phone. base and small are the on-device defaults; tiny is only for quick tests. On phones with 8 GB or more, small is the best default. Indonesian: avoid tiny — its Indonesian accuracy is noticeably worse; use base or small.";
pub const LANDING_ID: &str = "Pilih model untuk ponsel ini. base dan small adalah pilihan bawaan di perangkat; tiny hanya untuk uji cepat. Di ponsel dengan RAM 8 GB atau lebih, small adalah pilihan terbaik. Untuk bahasa Indonesia, hindari tiny — akurasinya untuk bahasa Indonesia jauh lebih rendah; gunakan base atau small.";

/// The landing copy for a UI language (`id` → Indonesian, everything else English).
pub fn landing_text(language: &str) -> &'static str {
    if is_indonesian(language) {
        LANDING_ID
    } else {
        LANDING_EN
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::find_model;

    const GB: u64 = 1_000_000_000;

    fn profile(total_gb: f64, low_memory: bool, arm64: bool) -> DeviceProfile {
        DeviceProfile::new((total_gb * GB as f64) as u64, GB, low_memory, arm64)
    }

    /// The owner's floor: a 4 GB phone defaults to `base`, a 6 GB phone also
    /// defaults to `base` but may choose `small`, and an 8 GB phone defaults
    /// to `small`. English and Indonesian agree on the model, never on `tiny`.
    #[test]
    fn tier_defaults_are_base_base_small_for_4_6_8_gb_in_both_languages() {
        let cases = [
            (3.7, PhoneTier::Low, "base"),
            (5.5, PhoneTier::Mid, "base"),
            (7.4, PhoneTier::High, "small"),
        ];
        for (ram_gb, expected_tier, expected_default) in cases {
            for language in ["en", "id", "id-ID"] {
                let recommendation = recommend(profile(ram_gb, false, true), language);
                assert_eq!(
                    recommendation.tier, expected_tier,
                    "{ram_gb} GB should be {expected_tier:?}"
                );
                assert_eq!(
                    recommendation.default_model_id,
                    Some(expected_default),
                    "{ram_gb} GB / {language}: the default must never be tiny"
                );
                assert!(
                    recommendation.allowed.contains(&expected_default),
                    "{ram_gb} GB / {language}: the default must be allowed"
                );
                assert!(recommendation.tier.meets_floor());
            }
        }
    }

    /// Small is offered from 6 GB, not from 4 GB; base is offered from 4 GB.
    #[test]
    fn allowed_sets_grow_with_the_tier() {
        let low = recommend(profile(3.7, false, true), "en");
        assert!(low.allowed.contains(&"base"));
        assert!(!low.allowed.contains(&"small"));
        assert!(low.refused.iter().any(|r| r.id == "small"));

        let mid = recommend(profile(5.5, false, true), "en");
        assert!(mid.allowed.contains(&"small"));
        assert_eq!(mid.default_model_id, Some("base"));

        let high = recommend(profile(7.4, false, true), "en");
        assert!(high.allowed.contains(&"turbo-q5-0"));
        assert!(!high.allowed.contains(&"turbo"));
        assert!(!high.allowed.contains(&"turbo-q8-0"));
    }

    /// Below 4 GB only `tiny` is allowed; every larger model is refused with a
    /// message that names the 4 GB requirement.
    #[test]
    fn below_the_floor_refuses_every_larger_model_with_a_clear_reason() {
        let recommendation = recommend(profile(2.7, false, true), "en");
        assert_eq!(recommendation.tier, PhoneTier::BelowFloor);
        assert!(!recommendation.tier.meets_floor());
        assert_eq!(recommendation.default_model_id, Some("tiny"));
        assert_eq!(recommendation.allowed, vec!["tiny"]);
        for id in ["base", "small", "turbo-q5-0"] {
            let refusal = recommendation
                .refused
                .iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("{id} must be refused below the floor"));
            assert!(
                refusal.reason.contains("4 GB"),
                "{id}: the refusal must name the floor: {}",
                refusal.reason
            );
        }
        // The two models that are never offered on any phone say so instead.
        for id in ["turbo-q8-0", "turbo"] {
            let refusal = recommendation.refused.iter().find(|r| r.id == id).unwrap();
            assert!(
                refusal.reason.contains("not offered on phones"),
                "{id}: {}",
                refusal.reason
            );
        }
        assert!(
            recommendation.detail.contains("2.7 GB"),
            "the detail must quote the device RAM: {}",
            recommendation.detail
        );
    }

    /// `isLowRamDevice` wins over a byte count that looks sufficient.
    #[test]
    fn the_platform_low_ram_flag_forces_the_below_floor_tier() {
        let flagged = DeviceProfile::new(4_000_000_000, 1_000_000_000, true, true);
        assert_eq!(tier_for(flagged), PhoneTier::BelowFloor);
        let same_without_flag = DeviceProfile::new(4_000_000_000, 1_000_000_000, false, true);
        assert_eq!(tier_for(same_without_flag), PhoneTier::Low);
    }

    /// A non-arm64 device runs nothing; the message says why.
    #[test]
    fn a_non_arm64_device_is_unsupported() {
        let recommendation = recommend(profile(8.0, false, false), "en");
        assert_eq!(recommendation.tier, PhoneTier::Unsupported);
        assert!(recommendation.allowed.is_empty());
        assert!(recommendation.default_model_id.is_none());
        assert_eq!(recommendation.refused.len(), MODELS.len());
        assert!(recommendation
            .refused
            .iter()
            .all(|r| r.reason.contains("64-bit ARM")));
        assert_eq!(
            refusal_for(profile(8.0, false, false), "en", "tiny").as_deref(),
            Some("Suramak needs a 64-bit ARM phone; this device's CPU is not supported.")
        );
    }

    /// Indonesian is steered away from tiny everywhere it could be chosen,
    /// while English is not nagged.
    #[test]
    fn indonesian_is_steered_away_from_tiny() {
        for ram in [3.7, 5.5, 7.4] {
            let id = recommend(profile(ram, false, true), "id");
            let note = id
                .language_note
                .as_deref()
                .unwrap_or_else(|| panic!("{ram} GB: Indonesian must carry a note"));
            assert!(note.contains("tiny"), "{ram} GB: {note}");
            assert!(note.contains("Indonesian"), "{ram} GB: {note}");
        }
        // English carries no Indonesian note.
        assert!(recommend(profile(7.4, false, true), "en")
            .language_note
            .is_none());
        // A regional/uppercase tag still counts as Indonesian.
        assert!(is_indonesian("ID_id"));
        assert!(is_indonesian(" id-ID "));
        assert!(!is_indonesian("ind"));
    }

    /// The manifest carries the phone rules, and every model has an explicit
    /// minimum: no model can silently inherit "runs anywhere".
    #[test]
    fn every_manifest_model_declares_phone_guidance() {
        for model in MODELS {
            assert!(
                !model.phone.note.is_empty(),
                "{}: the phone note is what a refusal shows",
                model.id
            );
            if model.phone.default_from_ram_bytes > 0 {
                assert!(
                    model.phone.default_from_ram_bytes >= model.phone.min_ram_bytes,
                    "{}: a default cannot start below its own minimum",
                    model.id
                );
            }
        }
        // The two phone defaults the plan names.
        assert!(find_model("base").unwrap().phone.default_from_ram_bytes > 0);
        assert!(find_model("small").unwrap().phone.default_from_ram_bytes > 0);
        // tiny is never a default on a device that meets the floor.
        assert_eq!(find_model("tiny").unwrap().phone.default_from_ram_bytes, 0);
    }

    /// The landing copy names the defaults and warns Indonesian users off tiny.
    #[test]
    fn landing_copy_is_honest_in_both_languages() {
        assert!(LANDING_EN.contains("base"));
        assert!(LANDING_EN.contains("small"));
        assert!(LANDING_EN.contains("tiny"));
        assert!(LANDING_EN.contains("Indonesian"));
        assert!(LANDING_ID.contains("base"));
        assert!(LANDING_ID.contains("small"));
        assert!(LANDING_ID.contains("tiny"));
        assert!(LANDING_ID.contains("Indonesia"));
        assert_eq!(landing_text("id-ID"), LANDING_ID);
        assert_eq!(landing_text("en"), LANDING_EN);
    }

    /// Refusal errors quote the model and the RAM; allowed models return none.
    #[test]
    fn refusal_for_only_refuses_models_the_device_cannot_run() {
        assert_eq!(refusal_for(profile(7.4, false, true), "en", "small"), None);
        assert_eq!(refusal_for(profile(3.7, false, true), "en", "base"), None);
        let refused = refusal_for(profile(2.7, false, true), "en", "small").unwrap();
        assert!(
            refused.contains("small") && refused.contains("4 GB"),
            "{refused}"
        );
        let too_big = refusal_for(profile(7.4, false, true), "en", "turbo").unwrap();
        assert!(too_big.contains("not offered on phones"), "{too_big}");
        assert_eq!(refusal_for(profile(7.4, false, true), "en", "nope"), None);
    }

    // ---- M19B: the phone's model choice at first run and after migration ----

    /// The tier default the phone selects when nothing else is installed:
    /// `base` on 4 and 6 GB, `small` on 8, 12 and 16 GB.
    #[test]
    fn the_phone_default_follows_the_tier_at_4_6_8_12_16_gb() {
        let cases = [
            (4.0, PhoneTier::Low, "base"),
            (6.0, PhoneTier::Mid, "base"),
            (8.0, PhoneTier::High, "small"),
            (12.0, PhoneTier::High, "small"),
            (16.0, PhoneTier::High, "small"),
        ];
        for (ram_gb, expected_tier, expected) in cases {
            let device = profile(ram_gb, false, true);
            assert_eq!(tier_for(device), expected_tier, "{ram_gb} GB");
            assert_eq!(
                recommend(device, "en").default_model_id,
                Some(expected),
                "{ram_gb} GB"
            );
            // The migration path names the same model for an untouched install.
            assert_eq!(
                model_choice_for_phone("turbo-q5-0", false, device, &[]),
                Some(expected),
                "{ram_gb} GB: a fresh phone takes its tier default"
            );
        }
    }

    /// Below the floor the existing steer is unchanged: `tiny`, and only tiny.
    #[test]
    fn below_the_floor_the_default_is_still_tiny() {
        let device = profile(2.7, false, true);
        assert_eq!(tier_for(device), PhoneTier::BelowFloor);
        assert_eq!(recommend(device, "en").default_model_id, Some("tiny"));
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, device, &[]),
            Some("tiny")
        );
    }

    /// The migration matrix: only an unchosen, unverified desktop default moves.
    #[test]
    fn the_migration_matrix() {
        let high = profile(7.4, false, true); // 8 GB class: tier default small
        let low = profile(3.7, false, true); // 4 GB class: tier default base

        // (a) Old file + turbo + nothing installed → the tier default.
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, high, &[]),
            Some("small")
        );
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, low, &[]),
            Some("base")
        );

        // (b) Old file + turbo + an installed model → the highest-tier
        // installed model, even when the tier default is something else.
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, high, &["base"]),
            Some("base")
        );
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, high, &["tiny", "base"]),
            Some("base")
        );
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, high, &["base", "small"]),
            Some("small")
        );
        // A model the phone may not run cannot win: small is refused on 4 GB.
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, low, &["base", "small"]),
            Some("base")
        );

        // (c) Turbo itself verified → keep it: it is a real file.
        assert_eq!(
            model_choice_for_phone("turbo-q5-0", false, high, &["turbo-q5-0"]),
            None
        );

        // (d) An explicit user choice is never overridden.
        assert_eq!(model_choice_for_phone("turbo-q5-0", true, high, &[]), None);
        // (e) A different stored model (a real choice made before the flag
        // existed) is kept as-is.
        assert_eq!(model_choice_for_phone("small", false, high, &[]), None);
        assert_eq!(model_choice_for_phone("tiny", false, low, &["base"]), None);
    }

    /// What "highest tier" means on disk, and what happens when nothing usable
    /// is installed.
    #[test]
    fn the_highest_verified_model_is_the_biggest_that_may_run() {
        let high = profile(7.4, false, true);
        assert_eq!(highest_tier_verified(high, &[]), None);
        assert_eq!(highest_tier_verified(high, &["tiny"]), Some("tiny"));
        assert_eq!(
            highest_tier_verified(high, &["tiny", "small", "base"]),
            Some("small")
        );
        assert_eq!(
            highest_tier_verified(high, &["turbo"]),
            None,
            "a verified file the phone may not run is not a choice"
        );
        let unsupported = profile(8.0, false, false);
        assert_eq!(highest_tier_verified(unsupported, &["tiny"]), None);
    }
}
