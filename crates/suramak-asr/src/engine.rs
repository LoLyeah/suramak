//! Engine selection: a **pure function** of
//! `(setting, fast engine, language, availability)` → engine id.
//!
//! Rationale (docs/ENGINES.md, multi-engine strategy): different engines per language is
//! the agreed end state (Whisper for English + Indonesian now; Parakeet and Apple
//! SpeechTranscriber later). Adding an engine must be a new [`EngineId`] variant plus a
//! routing rule here — never a reshape of the call sites, which always receive an
//! [`EngineDecision`] and always report the engine that actually ran.
//!
//! Whisper backends: `CoreML` (Core ML encoder on the ANE, macOS), `Metal` (GPU
//! decoder, macOS) and `Cpu`.
//!
//! M10A adds the **fast engine** — `Parakeet` (NVIDIA Parakeet TDT 0.6B v3 int8 via
//! sherpa-onnx, `docs/WORKORDER-M10A.md`). M10B adds the **macOS fast path** —
//! `SpeechTranscriber` (Apple's on-device SpeechAnalyzer pipeline, macOS 26+, via
//! the Rust `speech` crate, `docs/WORKORDER-M10B.md`). Both are opt-in twice over:
//! the build must carry the engine's cargo feature *and* the engine must be usable
//! *and* the "Fast engine (experimental)" setting must be on. Routing rules:
//!
//! - the fast engines apply only while the whisper `engine` setting is `Auto` — an
//!   explicit `coreml`/`metal`/`cpu` request means "run whisper on this backend";
//! - on macOS, a **covered** language with SpeechTranscriber available routes to
//!   SpeechTranscriber first (the macOS fast path); else the Parakeet check; else
//!   Whisper with a warning naming the blocker;
//! - only a **forced, covered** language routes to a fast engine (Parakeet: the
//!   list pinned in [`suramak_core::models::PARAKEET`]; SpeechTranscriber: the
//!   list pinned in [`suramak_core::models::SPEECH_TRANSCRIBER_LANGUAGES`]);
//! - **Indonesian never routes to either fast engine** (hard guards in
//!   [`suramak_core::models::parakeet_covers`] / [`suramak_core::models::speech_transcriber_covers`];
//!   neither list has `id` and the routing tests pin the invariant);
//! - **SpeechTranscriber is never selected off-macOS** (the `speech` crate is a
//!   macOS Swift bridge) and never for a language the pinned list does not cover;
//! - language auto-detect (`None`) stays Whisper: the language is unknown at plan
//!   time, so an Indonesian clip could reach a fast engine — the honest default is
//!   Whisper;
//! - everything else stays Whisper, exactly as today.

use std::path::Path;

use serde::{Deserialize, Serialize};
use suramak_core::models;
use suramak_core::settings::EngineSetting;

/// The engine that runs a transcription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EngineId {
    /// whisper.cpp with the Core ML encoder package loaded (ANE/GPU via Core ML).
    #[serde(rename = "coreml")]
    CoreML,
    /// whisper.cpp on the Metal backend.
    #[serde(rename = "metal")]
    Metal,
    /// whisper.cpp on CPU.
    #[serde(rename = "cpu")]
    Cpu,
    /// NVIDIA Parakeet TDT 0.6B v3 int8 through sherpa-onnx (M10A, opt-in).
    #[serde(rename = "parakeet")]
    Parakeet,
    /// Apple's on-device SpeechTranscriber (macOS 26+ SpeechAnalyzer pipeline) via
    /// the Rust `speech` crate (M10B, opt-in, macOS only).
    #[serde(rename = "speech-transcriber")]
    SpeechTranscriber,
}

impl EngineId {
    /// Lowercase identifier used in settings, CLI flags and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::CoreML => "coreml",
            EngineId::Metal => "metal",
            EngineId::Cpu => "cpu",
            EngineId::Parakeet => "parakeet",
            EngineId::SpeechTranscriber => "speech-transcriber",
        }
    }

    /// Whether this engine is a whisper.cpp backend (Parakeet and
    /// SpeechTranscriber are not — neither loads a whisper context).
    pub fn is_whisper(self) -> bool {
        !matches!(self, EngineId::Parakeet | EngineId::SpeechTranscriber)
    }

    /// Value for whisper.cpp's `use_gpu` context parameter.
    ///
    /// Metal and Core ML both keep the ggml decoder on the GPU (the Core ML package only
    /// replaces the *encoder*); CPU turns the GPU off. Parakeet and SpeechTranscriber are
    /// not whisper backends, so this is `false` and callers must not load a whisper
    /// context for them ([`EngineId::is_whisper`]).
    pub fn use_gpu(self) -> bool {
        matches!(self, EngineId::CoreML | EngineId::Metal)
    }
}

/// What the current build, platform and model install can offer.
///
/// Constructed with [`EngineAvailability::current`] in production and with literals in
/// tests, which is what makes engine selection testable on any platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineAvailability {
    /// macOS is the only platform with Metal and Core ML.
    pub macos: bool,
    /// The linked whisper.cpp was built with Core ML support (`COREML = 1`).
    pub coreml_compiled: bool,
    /// The linked whisper.cpp was built with Metal support.
    pub metal_compiled: bool,
    /// The Core ML encoder package for the selected model is installed.
    pub encoder_installed: bool,
    /// This build carries the `parakeet` cargo feature (sherpa-onnx linked).
    pub parakeet_compiled: bool,
    /// The Parakeet model directory is installed under the models dir.
    pub parakeet_installed: bool,
    /// This build carries the `speech-transcriber` feature **and** targets macOS
    /// (the `speech` crate is a Swift bridge — macOS only).
    pub speech_transcriber_compiled: bool,
    /// Runtime availability of Apple SpeechTranscriber for the *language this
    /// availability was computed for*: macOS 26+ with a working
    /// `SpeechTranscriber` **and** the OS supporting the language's locale
    /// (probed through the crate). Off-macOS or without the feature this is
    /// always `false`. A pure-function input — the tests pin the whole matrix.
    pub speech_transcriber_available: bool,
}

impl EngineAvailability {
    /// Availability for `model_path` on this platform/build, for `language`
    /// (the SpeechTranscriber probe is language-dependent — its locale must be
    /// supported by the OS; `None` means auto-detect, where the probe is `false`).
    ///
    /// The Parakeet model lives in the models dir next to the whisper model file
    /// (`model_path.parent()`), so a custom whisper path outside the models dir
    /// simply never selects the fast engine.
    pub fn current(model_path: &Path, language: Option<&str>) -> Self {
        let models_dir = model_path.parent().unwrap_or(Path::new("."));
        Self {
            macos: cfg!(target_os = "macos"),
            coreml_compiled: crate::whisper::coreml_compiled(),
            metal_compiled: crate::whisper::metal_compiled(),
            encoder_installed: models::encoder_installed(model_path),
            parakeet_compiled: crate::parakeet::compiled(),
            parakeet_installed: models::parakeet_installed(models_dir),
            speech_transcriber_compiled: crate::speech_transcriber::compiled(),
            speech_transcriber_available: crate::speech_transcriber::available_for(language),
        }
    }

    /// The best engine that is not Core ML: Metal when available, else CPU.
    pub fn best_without_coreml(&self) -> EngineId {
        if self.macos && self.metal_compiled {
            EngineId::Metal
        } else {
            EngineId::Cpu
        }
    }

    /// Whether the fast engine could run: feature compiled in **and** the model
    /// installed.
    pub fn parakeet_ready(&self) -> bool {
        self.parakeet_compiled && self.parakeet_installed
    }

    /// Whether Apple SpeechTranscriber could run: macOS, the feature compiled
    /// in, and the OS/locale probe positive. The static coverage list (and the
    /// Indonesian hard guard) is checked separately by the routing.
    pub fn speech_transcriber_ready(&self) -> bool {
        self.macos && self.speech_transcriber_compiled && self.speech_transcriber_available
    }
}

/// The engine that will run, plus a note when the request could not be honoured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDecision {
    /// Planned engine (the *engaged* engine can still differ if Core ML init fails at
    /// runtime — the pipeline reports that separately).
    pub engine: EngineId,
    /// Human-readable explanation when the request was downgraded.
    pub warning: Option<String>,
}

impl EngineDecision {
    fn new(engine: EngineId, warning: Option<String>) -> Self {
        Self { engine, warning }
    }
}

/// Why Core ML cannot be used, if it cannot.
fn coreml_blocker(avail: &EngineAvailability) -> Option<String> {
    if !avail.macos {
        return Some("Core ML is macOS-only".to_string());
    }
    if !avail.coreml_compiled {
        return Some("this build was compiled without the `coreml` feature".to_string());
    }
    if !avail.encoder_installed {
        return Some(
            "the Core ML encoder package for this model is not installed \
             (run `suramak-cli models download <id> --encoder`)"
                .to_string(),
        );
    }
    None
}

/// The whisper engine `Auto` resolves to.
fn auto_whisper(avail: &EngineAvailability) -> EngineId {
    if avail.macos && avail.coreml_compiled && avail.encoder_installed {
        EngineId::CoreML
    } else if avail.macos && avail.metal_compiled {
        EngineId::Metal
    } else {
        EngineId::Cpu
    }
}

/// Why the fast engine cannot run for a covered language, if it cannot.
fn parakeet_blocker(avail: &EngineAvailability) -> String {
    if !avail.parakeet_compiled {
        return "this build was compiled without the `parakeet` feature".to_string();
    }
    "the Parakeet model is not installed (run `suramak-cli models download parakeet`)".to_string()
}

/// Why Apple SpeechTranscriber cannot run for a covered language, if it cannot.
fn speech_transcriber_blocker(avail: &EngineAvailability) -> String {
    if !avail.macos {
        return "SpeechTranscriber is macOS-only".to_string();
    }
    if !avail.speech_transcriber_compiled {
        return "this build was compiled without the `speech-transcriber` feature".to_string();
    }
    "SpeechTranscriber is not available on this macOS version or for this locale".to_string()
}

/// Pure engine selection.
///
/// Rules:
/// - **Fast engines** — `fast` (the "Fast engine (experimental)" setting,
///   default OFF) with `Auto` and a forced language, in order:
///   1. macOS + a SpeechTranscriber-covered language (`speech_transcriber_covers`,
///      the Indonesian hard guard included) + `avail.speech_transcriber_ready()`
///      → [`EngineId::SpeechTranscriber`] (the macOS fast path);
///   2. else a Parakeet-covered language + `avail.parakeet_ready()` →
///      [`EngineId::Parakeet`] (M10A);
///   3. else Whisper — with a warning naming the blocker(s) when at least one
///      fast engine covers the language but cannot run.
///
/// Indonesian never routes to either fast engine (see the module docs);
/// auto-detect (`None`) never; an explicit whisper backend never; non-macOS
/// never selects SpeechTranscriber.
/// - `Cpu` → CPU, always.
/// - `Metal` → Metal on macOS builds with the Metal backend, else CPU (with a warning).
/// - `CoreML` → Core ML when it is available (macOS + `coreml` build + encoder package
///   installed), else the best non-Core ML engine (with a warning).
/// - `Auto` → on macOS: Core ML (only when the encoder package is installed) → Metal →
///   CPU; on other platforms CPU. No warning: this *is* the rule.
pub fn resolve_engine(
    setting: EngineSetting,
    fast: bool,
    language: Option<&str>,
    avail: &EngineAvailability,
) -> EngineDecision {
    if fast && setting == EngineSetting::Auto {
        if let Some(code) = language {
            let st_covers = models::speech_transcriber_covers(code);
            if st_covers && avail.speech_transcriber_ready() {
                return EngineDecision::new(EngineId::SpeechTranscriber, None);
            }
            let parakeet_covers = models::parakeet_covers(code);
            if parakeet_covers {
                if avail.parakeet_ready() {
                    return EngineDecision::new(EngineId::Parakeet, None);
                }
                // Neither fast engine can run for this covered language: name
                // every blocker that applies.
                let whisper = auto_whisper(avail);
                let mut blockers = Vec::new();
                if st_covers {
                    blockers.push(speech_transcriber_blocker(avail));
                }
                blockers.push(parakeet_blocker(avail));
                return EngineDecision::new(
                    whisper,
                    Some(format!(
                        "fast engine requested but {} — using {}",
                        blockers.join("; "),
                        whisper.as_str()
                    )),
                );
            }
            if st_covers {
                // Only SpeechTranscriber covers this language (ja/ko/zh/yue on
                // this tree): the Parakeet check does not apply.
                let whisper = auto_whisper(avail);
                return EngineDecision::new(
                    whisper,
                    Some(format!(
                        "fast engine requested but {} — using {}",
                        speech_transcriber_blocker(avail),
                        whisper.as_str()
                    )),
                );
            }
        }
    }
    match setting {
        EngineSetting::Cpu => EngineDecision::new(EngineId::Cpu, None),
        EngineSetting::Metal => {
            if avail.macos && avail.metal_compiled {
                EngineDecision::new(EngineId::Metal, None)
            } else {
                let warning = if !avail.macos {
                    "Metal is macOS-only"
                } else {
                    "this build was compiled without the `metal` feature"
                };
                EngineDecision::new(EngineId::Cpu, Some(format!("{warning} — using cpu")))
            }
        }
        EngineSetting::CoreML => match coreml_blocker(avail) {
            None => EngineDecision::new(EngineId::CoreML, None),
            Some(reason) => {
                let fallback = avail.best_without_coreml();
                EngineDecision::new(
                    fallback,
                    Some(format!(
                        "Core ML requested but {reason} — using {}",
                        fallback.as_str()
                    )),
                )
            }
        },
        EngineSetting::Auto => EngineDecision::new(auto_whisper(avail), None),
    }
}

/// Resolves the engine for `model_path` under the current build/platform.
pub fn resolve_for(
    model_path: &Path,
    setting: EngineSetting,
    fast: bool,
    language: Option<&str>,
) -> EngineDecision {
    resolve_engine(
        setting,
        fast,
        language,
        &EngineAvailability::current(model_path, language),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn avail(macos: bool, coreml: bool, metal: bool, encoder: bool) -> EngineAvailability {
        EngineAvailability {
            macos,
            coreml_compiled: coreml,
            metal_compiled: metal,
            encoder_installed: encoder,
            parakeet_compiled: false,
            parakeet_installed: false,
            speech_transcriber_compiled: false,
            speech_transcriber_available: false,
        }
    }

    /// The same availability with the fast engine compiled in and installed.
    fn with_parakeet(mut base: EngineAvailability) -> EngineAvailability {
        base.parakeet_compiled = true;
        base.parakeet_installed = true;
        base
    }

    /// The same availability with SpeechTranscriber compiled in and the OS/locale
    /// probe positive.
    fn with_speech_transcriber(mut base: EngineAvailability) -> EngineAvailability {
        base.speech_transcriber_compiled = true;
        base.speech_transcriber_available = true;
        base
    }

    /// macOS build with everything wired and the encoder package installed.
    const MAC_FULL: EngineAvailability = EngineAvailability {
        macos: true,
        coreml_compiled: true,
        metal_compiled: true,
        encoder_installed: true,
        parakeet_compiled: false,
        parakeet_installed: false,
        speech_transcriber_compiled: false,
        speech_transcriber_available: false,
    };
    /// macOS build, encoder package not downloaded yet.
    const MAC_NO_ENCODER: EngineAvailability = EngineAvailability {
        macos: true,
        coreml_compiled: true,
        metal_compiled: true,
        encoder_installed: false,
        parakeet_compiled: false,
        parakeet_installed: false,
        speech_transcriber_compiled: false,
        speech_transcriber_available: false,
    };
    /// macOS build without the `coreml` feature.
    const MAC_METAL_ONLY: EngineAvailability = EngineAvailability {
        macos: true,
        coreml_compiled: false,
        metal_compiled: true,
        encoder_installed: true,
        parakeet_compiled: false,
        parakeet_installed: false,
        speech_transcriber_compiled: false,
        speech_transcriber_available: false,
    };
    /// Linux/Windows build.
    const OTHER: EngineAvailability = EngineAvailability {
        macos: false,
        coreml_compiled: false,
        metal_compiled: false,
        encoder_installed: false,
        parakeet_compiled: false,
        parakeet_installed: false,
        speech_transcriber_compiled: false,
        speech_transcriber_available: false,
    };

    #[test]
    fn auto_on_macos_prefers_coreml_then_metal_then_cpu() {
        assert_eq!(
            resolve_engine(EngineSetting::Auto, false, None, &MAC_FULL).engine,
            EngineId::CoreML
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, false, None, &MAC_NO_ENCODER).engine,
            EngineId::Metal
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, false, None, &MAC_METAL_ONLY).engine,
            EngineId::Metal
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, false, None, &OTHER).engine,
            EngineId::Cpu
        );
        // Auto never warns: it picks what the machine can do.
        for a in [MAC_FULL, MAC_NO_ENCODER, MAC_METAL_ONLY, OTHER] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, false, None, &a).warning,
                None
            );
        }
    }

    #[test]
    fn auto_uses_cpu_on_non_macos_even_with_coreml_compiled() {
        // A (hypothetical) non-macOS build with the coreml feature must not try Core ML.
        let odd = avail(false, true, true, true);
        assert_eq!(
            resolve_engine(EngineSetting::Auto, false, None, &odd).engine,
            EngineId::Cpu
        );
    }

    #[test]
    fn explicit_coreml_downgrades_with_a_reason() {
        let decision = resolve_engine(EngineSetting::CoreML, false, None, &OTHER);
        assert_eq!(decision.engine, EngineId::Cpu);
        let warning = decision.warning.expect("downgrade must be explained");
        assert!(warning.contains("macOS-only"), "unexpected: {warning}");

        let decision = resolve_engine(EngineSetting::CoreML, false, None, &MAC_NO_ENCODER);
        assert_eq!(decision.engine, EngineId::Metal);
        let warning = decision.warning.expect("downgrade must be explained");
        assert!(warning.contains("--encoder"), "unexpected: {warning}");

        let decision = resolve_engine(EngineSetting::CoreML, false, None, &MAC_METAL_ONLY);
        assert_eq!(decision.engine, EngineId::Metal);
        assert!(decision
            .warning
            .expect("downgrade must be explained")
            .contains("coreml"));

        // Without Metal the downgrade lands on CPU.
        let mac_cpu_only = avail(true, false, false, false);
        let decision = resolve_engine(EngineSetting::CoreML, false, None, &mac_cpu_only);
        assert_eq!(decision.engine, EngineId::Cpu);
        assert!(decision.warning.is_some());
    }

    #[test]
    fn explicit_coreml_is_honoured_when_available() {
        let decision = resolve_engine(EngineSetting::CoreML, false, Some("id"), &MAC_FULL);
        assert_eq!(decision.engine, EngineId::CoreML);
        assert_eq!(decision.warning, None);
    }

    #[test]
    fn explicit_metal_and_cpu() {
        assert_eq!(
            resolve_engine(EngineSetting::Metal, false, None, &MAC_FULL).engine,
            EngineId::Metal
        );
        let decision = resolve_engine(EngineSetting::Metal, false, None, &OTHER);
        assert_eq!(decision.engine, EngineId::Cpu);
        assert!(decision.warning.is_some());

        for a in [MAC_FULL, MAC_NO_ENCODER, OTHER] {
            let decision = resolve_engine(EngineSetting::Cpu, false, None, &a);
            assert_eq!(decision.engine, EngineId::Cpu);
            assert_eq!(decision.warning, None);
        }
    }

    #[test]
    fn language_does_not_change_todays_routing_but_is_accepted() {
        // Without the fast engine every language resolves identically.
        for lang in [None, Some("en"), Some("id")] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, false, lang, &MAC_FULL).engine,
                EngineId::CoreML
            );
            assert_eq!(
                resolve_engine(EngineSetting::Auto, false, lang, &OTHER).engine,
                EngineId::Cpu
            );
        }
    }

    #[test]
    fn use_gpu_matches_the_engine() {
        assert!(EngineId::CoreML.use_gpu());
        assert!(EngineId::Metal.use_gpu());
        assert!(!EngineId::Cpu.use_gpu());
        assert!(!EngineId::Parakeet.use_gpu());
        assert!(!EngineId::SpeechTranscriber.use_gpu());
        assert!(EngineId::Cpu.is_whisper());
        assert!(!EngineId::Parakeet.is_whisper());
        assert!(!EngineId::SpeechTranscriber.is_whisper());
        assert_eq!(EngineId::CoreML.as_str(), "coreml");
        assert_eq!(EngineId::Metal.as_str(), "metal");
        assert_eq!(EngineId::Cpu.as_str(), "cpu");
        assert_eq!(EngineId::Parakeet.as_str(), "parakeet");
        assert_eq!(EngineId::SpeechTranscriber.as_str(), "speech-transcriber");
    }

    #[test]
    fn engine_ids_serialize_as_lowercase_names() {
        for (id, text) in [
            (EngineId::CoreML, "\"coreml\""),
            (EngineId::Metal, "\"metal\""),
            (EngineId::Cpu, "\"cpu\""),
            (EngineId::Parakeet, "\"parakeet\""),
            (EngineId::SpeechTranscriber, "\"speech-transcriber\""),
        ] {
            assert_eq!(serde_json::to_string(&id).unwrap(), text);
        }
    }

    #[test]
    fn availability_best_without_coreml() {
        assert_eq!(MAC_FULL.best_without_coreml(), EngineId::Metal);
        assert_eq!(OTHER.best_without_coreml(), EngineId::Cpu);
        assert_eq!(
            avail(true, false, false, false).best_without_coreml(),
            EngineId::Cpu
        );
    }

    // ---- M10A: the fast engine routing matrix ------------------------------

    #[test]
    fn fast_engine_routes_covered_languages_to_parakeet_when_ready() {
        let mac = with_parakeet(MAC_FULL);
        let linux = with_parakeet(OTHER);
        for a in [mac, linux] {
            for lang in ["en", "de", "fr", "es", "pt-BR", "EN"] {
                let decision = resolve_engine(EngineSetting::Auto, true, Some(lang), &a);
                assert_eq!(
                    decision.engine,
                    EngineId::Parakeet,
                    "{lang} must route to Parakeet on {a:?}"
                );
                assert_eq!(decision.warning, None);
            }
        }
    }

    #[test]
    fn fast_engine_never_routes_indonesian_to_parakeet() {
        // The M10A invariant, over the whole availability matrix.
        let matrix = [
            with_parakeet(MAC_FULL),
            with_parakeet(MAC_NO_ENCODER),
            with_parakeet(MAC_METAL_ONLY),
            with_parakeet(OTHER),
            MAC_FULL,
            OTHER,
        ];
        for a in matrix {
            for setting in [
                EngineSetting::Auto,
                EngineSetting::Cpu,
                EngineSetting::CoreML,
            ] {
                for lang in ["id", "ID", "id-ID", "id-id"] {
                    let decision = resolve_engine(setting, true, Some(lang), &a);
                    assert_ne!(
                        decision.engine,
                        EngineId::Parakeet,
                        "Indonesian must never select Parakeet ({setting:?}, {lang}, {a:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn fast_engine_leaves_uncovered_languages_and_auto_detect_on_whisper() {
        let mac = with_parakeet(MAC_FULL);
        let linux = with_parakeet(OTHER);
        // Languages neither fast engine covers.
        for lang in ["ar", "jv", "xx", "th"] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, true, Some(lang), &mac).engine,
                EngineId::CoreML,
                "{lang}"
            );
            assert_eq!(
                resolve_engine(EngineSetting::Auto, true, Some(lang), &linux).engine,
                EngineId::Cpu,
                "{lang}"
            );
        }
        // Language auto-detect: unknown at plan time → Whisper (an Indonesian clip
        // could otherwise reach a fast engine).
        for a in [mac, linux] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, true, None, &a).engine,
                if a.macos {
                    EngineId::CoreML
                } else {
                    EngineId::Cpu
                }
            );
        }
    }

    #[test]
    fn fast_engine_is_off_by_default_and_never_overrides_an_explicit_backend() {
        let a = with_parakeet(MAC_FULL);
        // `fast = false` (the default) keeps today's routing for every language.
        for lang in [None, Some("en"), Some("id")] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, false, lang, &a).engine,
                EngineId::CoreML
            );
        }
        // An explicit whisper backend request wins over the fast engine.
        for setting in [
            EngineSetting::Cpu,
            EngineSetting::Metal,
            EngineSetting::CoreML,
        ] {
            let decision = resolve_engine(setting, true, Some("en"), &a);
            assert_ne!(decision.engine, EngineId::Parakeet, "{setting:?}");
        }
        assert_eq!(
            resolve_engine(EngineSetting::Cpu, true, Some("en"), &a).engine,
            EngineId::Cpu
        );
    }

    #[test]
    fn fast_engine_without_the_model_warns_and_falls_back_to_whisper() {
        // Compiled in, not installed.
        let mut not_installed = with_parakeet(OTHER);
        not_installed.parakeet_installed = false;
        let decision = resolve_engine(EngineSetting::Auto, true, Some("en"), &not_installed);
        assert_eq!(decision.engine, EngineId::Cpu);
        let warning = decision.warning.expect("the miss must be explained");
        assert!(
            warning.contains("not installed") && warning.contains("models download parakeet"),
            "unexpected: {warning}"
        );

        // Installed, but the build has no `parakeet` feature.
        let mut not_compiled = OTHER;
        not_compiled.parakeet_installed = true;
        let decision = resolve_engine(EngineSetting::Auto, true, Some("en"), &not_compiled);
        assert_eq!(decision.engine, EngineId::Cpu);
        let warning = decision.warning.expect("the miss must be explained");
        assert!(
            warning.contains("without the `parakeet` feature"),
            "unexpected: {warning}"
        );

        // Uncovered languages and auto-detect never warn (that *is* the rule).
        // ("ar" is covered by neither fast engine; ja/ko/zh are
        // SpeechTranscriber-covered and warn when it cannot run — see the M10B
        // matrix below.)
        let a = with_parakeet(OTHER);
        assert_eq!(
            resolve_engine(EngineSetting::Auto, true, Some("id"), &a).warning,
            None
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, true, Some("ar"), &a).warning,
            None
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, true, None, &a).warning,
            None
        );
        // …and neither does an explicit backend with the fast engine on.
        assert_eq!(
            resolve_engine(EngineSetting::Cpu, true, Some("en"), &a).warning,
            None
        );
    }

    #[test]
    fn fast_engine_downgrades_to_the_whisper_rules_when_it_cannot_run() {
        // The whisper fallback follows the *whisper* rules: Core ML → Metal → CPU.
        let mut no_encoder = with_parakeet(MAC_NO_ENCODER);
        no_encoder.parakeet_installed = false;
        let decision = resolve_engine(EngineSetting::Auto, true, Some("en"), &no_encoder);
        assert_eq!(decision.engine, EngineId::Metal);
        assert!(decision.warning.is_some());

        let mut not_installed = with_parakeet(MAC_FULL);
        not_installed.parakeet_installed = false;
        assert_eq!(
            resolve_engine(EngineSetting::Auto, true, Some("en"), &not_installed).engine,
            EngineId::CoreML
        );

        // And when the fast engine *can* run on a full macOS setup, it wins.
        assert_eq!(
            resolve_engine(
                EngineSetting::Auto,
                true,
                Some("en"),
                &with_parakeet(MAC_FULL)
            )
            .engine,
            EngineId::Parakeet
        );
    }

    // ---- M10B: Apple SpeechTranscriber routing matrix -----------------------

    #[test]
    fn speech_transcriber_beats_parakeet_on_macos_when_both_are_ready() {
        let both = with_speech_transcriber(with_parakeet(MAC_FULL));
        for lang in [
            "en", "de", "fr", "es", "it", "pt", "pt-BR", "ja", "ko", "zh", "yue", "EN",
        ] {
            let decision = resolve_engine(EngineSetting::Auto, true, Some(lang), &both);
            assert_eq!(
                decision.engine,
                EngineId::SpeechTranscriber,
                "{lang} must take the macOS fast path"
            );
            assert_eq!(decision.warning, None, "{lang}");
        }
        // SpeechTranscriber alone (no Parakeet compiled/installed) still wins on
        // its covered languages.
        let st_only = with_speech_transcriber(MAC_FULL);
        for lang in ["en", "ja"] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, true, Some(lang), &st_only).engine,
                EngineId::SpeechTranscriber,
                "{lang}"
            );
        }
    }

    #[test]
    fn speech_transcriber_is_never_selected_off_macos() {
        // Even a (hypothetical) non-macOS availability claiming compiled +
        // available must never select it — the `speech` crate is macOS-only.
        let odd = with_speech_transcriber(with_parakeet(OTHER));
        for lang in ["en", "de", "ja"] {
            let decision = resolve_engine(EngineSetting::Auto, true, Some(lang), &odd);
            assert_ne!(decision.engine, EngineId::SpeechTranscriber, "{lang}");
        }
        // With Parakeet ready the covered language falls to Parakeet…
        assert_eq!(
            resolve_engine(EngineSetting::Auto, true, Some("en"), &odd).engine,
            EngineId::Parakeet
        );
        // …and a SpeechTranscriber-only language falls to Whisper with the
        // blocker named.
        let decision = resolve_engine(EngineSetting::Auto, true, Some("ja"), &odd);
        assert_eq!(decision.engine, EngineId::Cpu);
        let warning = decision.warning.expect("the miss must be explained");
        assert!(warning.contains("macOS-only"), "unexpected: {warning}");
    }

    #[test]
    fn speech_transcriber_never_routes_indonesian() {
        // The M10B invariant, over the whole availability matrix (incl. a
        // SpeechTranscriber-ready macOS build and the odd non-macOS one).
        let matrix = [
            with_speech_transcriber(with_parakeet(MAC_FULL)),
            with_speech_transcriber(with_parakeet(MAC_NO_ENCODER)),
            with_speech_transcriber(MAC_METAL_ONLY),
            with_speech_transcriber(with_parakeet(OTHER)),
            with_speech_transcriber(OTHER),
            with_parakeet(MAC_FULL),
            MAC_FULL,
            OTHER,
        ];
        for a in matrix {
            for setting in [
                EngineSetting::Auto,
                EngineSetting::Cpu,
                EngineSetting::CoreML,
                EngineSetting::Metal,
            ] {
                for lang in ["id", "ID", "id-ID", "id-id"] {
                    let decision = resolve_engine(setting, true, Some(lang), &a);
                    assert_ne!(
                        decision.engine,
                        EngineId::SpeechTranscriber,
                        "Indonesian must never select SpeechTranscriber ({setting:?}, {lang}, {a:?})"
                    );
                    assert_ne!(
                        decision.engine,
                        EngineId::Parakeet,
                        "Indonesian must never select Parakeet ({setting:?}, {lang}, {a:?})"
                    );
                }
            }
        }
    }

    #[test]
    fn speech_transcriber_blocked_falls_through_to_parakeet_then_whisper() {
        // macOS, SpeechTranscriber compiled but the OS/locale probe negative
        // (e.g. macOS 25 or a locale the OS lacks), Parakeet ready → Parakeet.
        let mut st_compiled_unavailable = with_parakeet(MAC_FULL);
        st_compiled_unavailable.speech_transcriber_compiled = true;
        st_compiled_unavailable.speech_transcriber_available = false;
        let decision = resolve_engine(
            EngineSetting::Auto,
            true,
            Some("en"),
            &st_compiled_unavailable,
        );
        assert_eq!(decision.engine, EngineId::Parakeet);
        assert_eq!(decision.warning, None);

        // Parakeet not ready either (compiled in, model missing) → Whisper
        // (Core ML here) with both blockers named.
        let mut st_compiled_unavailable = with_parakeet(MAC_FULL);
        st_compiled_unavailable.speech_transcriber_compiled = true;
        st_compiled_unavailable.parakeet_installed = false;
        let decision = resolve_engine(
            EngineSetting::Auto,
            true,
            Some("en"),
            &st_compiled_unavailable,
        );
        assert_eq!(decision.engine, EngineId::CoreML);
        let warning = decision.warning.expect("the miss must be explained");
        assert!(
            warning.contains("not available on this macOS version"),
            "unexpected: {warning}"
        );
        assert!(
            warning.contains("Parakeet model is not installed"),
            "unexpected: {warning}"
        );

        // A SpeechTranscriber-only language (Parakeet does not cover it) names
        // only the SpeechTranscriber blocker.
        let decision = resolve_engine(
            EngineSetting::Auto,
            true,
            Some("ja"),
            &st_compiled_unavailable,
        );
        assert_eq!(decision.engine, EngineId::CoreML);
        let warning = decision.warning.expect("the miss must be explained");
        assert!(
            warning.contains("not available on this macOS version"),
            "unexpected: {warning}"
        );
        assert!(
            !warning.contains("Parakeet"),
            "ja is not a Parakeet language: {warning}"
        );
    }

    #[test]
    fn speech_transcriber_requires_the_feature_and_a_mac() {
        // macOS + probe positive but the build has no `speech-transcriber`
        // feature → never selected; the blocker names the feature.
        let mut no_feature = MAC_FULL;
        no_feature.speech_transcriber_available = true;
        let decision = resolve_engine(EngineSetting::Auto, true, Some("en"), &no_feature);
        assert_ne!(decision.engine, EngineId::SpeechTranscriber);
        let warning = decision.warning.expect("the miss must be explained");
        assert!(
            warning.contains("without the `speech-transcriber` feature"),
            "unexpected: {warning}"
        );

        // `speech_transcriber_ready` respects every input.
        assert!(with_speech_transcriber(MAC_FULL).speech_transcriber_ready());
        assert!(!with_speech_transcriber(OTHER).speech_transcriber_ready());
        assert!(!MAC_FULL.speech_transcriber_ready());
        let mut compiled_only = MAC_FULL;
        compiled_only.speech_transcriber_compiled = true;
        assert!(!compiled_only.speech_transcriber_ready());
    }

    #[test]
    fn speech_transcriber_never_overrides_an_explicit_backend_or_the_toggle() {
        let both = with_speech_transcriber(with_parakeet(MAC_FULL));
        // Toggle OFF → today's routing for every language.
        for lang in [None, Some("en"), Some("id"), Some("ja")] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, false, lang, &both).engine,
                EngineId::CoreML
            );
        }
        // An explicit whisper backend wins over the fast path.
        for (setting, expected) in [
            (EngineSetting::Cpu, EngineId::Cpu),
            (EngineSetting::Metal, EngineId::Metal),
            (EngineSetting::CoreML, EngineId::CoreML),
        ] {
            let decision = resolve_engine(setting, true, Some("en"), &both);
            assert_eq!(decision.engine, expected, "{setting:?}");
            assert_eq!(decision.warning, None, "{setting:?}");
        }
    }
}
