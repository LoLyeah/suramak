//! Engine selection: a **pure function** of `(setting, language, availability)` → engine id.
//!
//! Rationale (docs/ENGINES.md, multi-engine strategy): different engines per language is
//! the agreed end state (Whisper for English + Indonesian now; Parakeet and Apple
//! SpeechTranscriber later). Adding an engine must be a new [`EngineId`] variant plus a
//! routing rule here — never a reshape of the call sites, which always receive an
//! [`EngineDecision`] and always report the engine that actually ran.
//!
//! Today only Whisper backends exist: `CoreML` (Core ML encoder on the ANE, macOS),
//! `Metal` (GPU decoder, macOS) and `Cpu`.

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
}

impl EngineId {
    /// Lowercase identifier used in settings, CLI flags and diagnostics.
    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::CoreML => "coreml",
            EngineId::Metal => "metal",
            EngineId::Cpu => "cpu",
        }
    }

    /// Value for whisper.cpp's `use_gpu` context parameter.
    ///
    /// Metal and Core ML both keep the ggml decoder on the GPU (the Core ML package only
    /// replaces the *encoder*); CPU turns the GPU off.
    pub fn use_gpu(self) -> bool {
        !matches!(self, EngineId::Cpu)
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
}

impl EngineAvailability {
    /// Availability for `model_path` on this platform/build.
    pub fn current(model_path: &Path) -> Self {
        Self {
            macos: cfg!(target_os = "macos"),
            coreml_compiled: crate::whisper::coreml_compiled(),
            metal_compiled: crate::whisper::metal_compiled(),
            encoder_installed: models::encoder_installed(model_path),
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

/// Pure engine selection.
///
/// Rules:
/// - `Cpu` → CPU, always.
/// - `Metal` → Metal on macOS builds with the Metal backend, else CPU (with a warning).
/// - `CoreML` → Core ML when it is available (macOS + `coreml` build + encoder package
///   installed), else the best non-Core ML engine (with a warning).
/// - `Auto` → on macOS: Core ML (only when the encoder package is installed) → Metal →
///   CPU; on other platforms CPU. No warning: this *is* the rule.
///
/// `language` is part of the contract because per-language routing (e.g. English →
/// Parakeet when installed) is the agreed direction; today every language routes to
/// Whisper, so it does not change the outcome.
pub fn resolve_engine(
    setting: EngineSetting,
    language: Option<&str>,
    avail: &EngineAvailability,
) -> EngineDecision {
    let _ = language;
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
        EngineSetting::Auto => {
            let engine = if avail.macos && avail.coreml_compiled && avail.encoder_installed {
                EngineId::CoreML
            } else if avail.macos && avail.metal_compiled {
                EngineId::Metal
            } else {
                EngineId::Cpu
            };
            EngineDecision::new(engine, None)
        }
    }
}

/// Resolves the engine for `model_path` under the current build/platform.
pub fn resolve_for(
    model_path: &Path,
    setting: EngineSetting,
    language: Option<&str>,
) -> EngineDecision {
    resolve_engine(setting, language, &EngineAvailability::current(model_path))
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
        }
    }

    /// macOS build with everything wired and the encoder package installed.
    const MAC_FULL: EngineAvailability = EngineAvailability {
        macos: true,
        coreml_compiled: true,
        metal_compiled: true,
        encoder_installed: true,
    };
    /// macOS build, encoder package not downloaded yet.
    const MAC_NO_ENCODER: EngineAvailability = EngineAvailability {
        macos: true,
        coreml_compiled: true,
        metal_compiled: true,
        encoder_installed: false,
    };
    /// macOS build without the `coreml` feature.
    const MAC_METAL_ONLY: EngineAvailability = EngineAvailability {
        macos: true,
        coreml_compiled: false,
        metal_compiled: true,
        encoder_installed: true,
    };
    /// Linux/Windows build.
    const OTHER: EngineAvailability = EngineAvailability {
        macos: false,
        coreml_compiled: false,
        metal_compiled: false,
        encoder_installed: false,
    };

    #[test]
    fn auto_on_macos_prefers_coreml_then_metal_then_cpu() {
        assert_eq!(
            resolve_engine(EngineSetting::Auto, None, &MAC_FULL).engine,
            EngineId::CoreML
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, None, &MAC_NO_ENCODER).engine,
            EngineId::Metal
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, None, &MAC_METAL_ONLY).engine,
            EngineId::Metal
        );
        assert_eq!(
            resolve_engine(EngineSetting::Auto, None, &OTHER).engine,
            EngineId::Cpu
        );
        // Auto never warns: it picks what the machine can do.
        for a in [MAC_FULL, MAC_NO_ENCODER, MAC_METAL_ONLY, OTHER] {
            assert_eq!(resolve_engine(EngineSetting::Auto, None, &a).warning, None);
        }
    }

    #[test]
    fn auto_uses_cpu_on_non_macos_even_with_coreml_compiled() {
        // A (hypothetical) non-macOS build with the coreml feature must not try Core ML.
        let odd = avail(false, true, true, true);
        assert_eq!(
            resolve_engine(EngineSetting::Auto, None, &odd).engine,
            EngineId::Cpu
        );
    }

    #[test]
    fn explicit_coreml_downgrades_with_a_reason() {
        let decision = resolve_engine(EngineSetting::CoreML, None, &OTHER);
        assert_eq!(decision.engine, EngineId::Cpu);
        let warning = decision.warning.expect("downgrade must be explained");
        assert!(warning.contains("macOS-only"), "unexpected: {warning}");

        let decision = resolve_engine(EngineSetting::CoreML, None, &MAC_NO_ENCODER);
        assert_eq!(decision.engine, EngineId::Metal);
        let warning = decision.warning.expect("downgrade must be explained");
        assert!(warning.contains("--encoder"), "unexpected: {warning}");

        let decision = resolve_engine(EngineSetting::CoreML, None, &MAC_METAL_ONLY);
        assert_eq!(decision.engine, EngineId::Metal);
        assert!(decision
            .warning
            .expect("downgrade must be explained")
            .contains("coreml"));

        // Without Metal the downgrade lands on CPU.
        let mac_cpu_only = avail(true, false, false, false);
        let decision = resolve_engine(EngineSetting::CoreML, None, &mac_cpu_only);
        assert_eq!(decision.engine, EngineId::Cpu);
        assert!(decision.warning.is_some());
    }

    #[test]
    fn explicit_coreml_is_honoured_when_available() {
        let decision = resolve_engine(EngineSetting::CoreML, Some("id"), &MAC_FULL);
        assert_eq!(decision.engine, EngineId::CoreML);
        assert_eq!(decision.warning, None);
    }

    #[test]
    fn explicit_metal_and_cpu() {
        assert_eq!(
            resolve_engine(EngineSetting::Metal, None, &MAC_FULL).engine,
            EngineId::Metal
        );
        let decision = resolve_engine(EngineSetting::Metal, None, &OTHER);
        assert_eq!(decision.engine, EngineId::Cpu);
        assert!(decision.warning.is_some());

        for a in [MAC_FULL, MAC_NO_ENCODER, OTHER] {
            let decision = resolve_engine(EngineSetting::Cpu, None, &a);
            assert_eq!(decision.engine, EngineId::Cpu);
            assert_eq!(decision.warning, None);
        }
    }

    #[test]
    fn language_does_not_change_todays_routing_but_is_accepted() {
        // Phase 2 will route `en` to Parakeet; today every language resolves identically.
        for lang in [None, Some("en"), Some("id")] {
            assert_eq!(
                resolve_engine(EngineSetting::Auto, lang, &MAC_FULL).engine,
                EngineId::CoreML
            );
            assert_eq!(
                resolve_engine(EngineSetting::Auto, lang, &OTHER).engine,
                EngineId::Cpu
            );
        }
    }

    #[test]
    fn use_gpu_matches_the_engine() {
        assert!(EngineId::CoreML.use_gpu());
        assert!(EngineId::Metal.use_gpu());
        assert!(!EngineId::Cpu.use_gpu());
        assert_eq!(EngineId::CoreML.as_str(), "coreml");
        assert_eq!(EngineId::Metal.as_str(), "metal");
        assert_eq!(EngineId::Cpu.as_str(), "cpu");
    }

    #[test]
    fn engine_ids_serialize_as_lowercase_names() {
        for (id, text) in [
            (EngineId::CoreML, "\"coreml\""),
            (EngineId::Metal, "\"metal\""),
            (EngineId::Cpu, "\"cpu\""),
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
}
