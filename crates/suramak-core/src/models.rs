//! Model manifest (ggml decoder files + optional Core ML encoder packages).

use std::path::{Path, PathBuf};

/// An official Core ML encoder package (`ggml-<model>-encoder.mlmodelc.zip`).
///
/// Encoder packages are fp16 and external to the ggml decoder, so a quantized decoder
/// (e.g. `turbo-q5-0`) pairs with the same package as its unquantized sibling.
#[derive(Debug, Clone, Copy)]
pub struct EncoderPackage {
    /// HuggingFace file name of the encoder zip.
    pub zip_file: &'static str,
    /// Size of the zip in bytes (pinned alongside the sha256).
    pub size_bytes: u64,
    /// sha256 of the zip.
    pub sha256: &'static str,
}

/// A model available for download from HuggingFace.
#[derive(Debug, Clone, Copy)]
pub struct ModelInfo {
    pub id: &'static str,
    pub hf_file: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    pub role: &'static str,
    /// Official Core ML encoder package; `None` when upstream ships no encoder for
    /// this model (every model in this manifest has one).
    pub encoder: Option<EncoderPackage>,
}

/// HuggingFace base URL for the whisper.cpp model repository.
pub const HF_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

/// Mirror base URL selected via `SURAMAK_HF_MIRROR` env var.
pub const HF_MIRROR_BASE_URL: &str = "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/";

/// The Core ML encoder package for `large-v3-turbo` (shared by all three turbo decoders:
/// the encoder is external to the ggml file, so `turbo`, `turbo-q5-0` and `turbo-q8-0`
/// all pair with it).
const LARGE_V3_TURBO_ENCODER: EncoderPackage = EncoderPackage {
    zip_file: "ggml-large-v3-turbo-encoder.mlmodelc.zip",
    size_bytes: 1_173_393_014,
    sha256: "84bedfe895bd7b5de6e8e89a0803dfc5addf8c0c5bc4c937451716bf7cf7988a",
};

/// All supported models. ggml values pasted verbatim from docs/MODEL-PINS.md; encoder
/// values pinned from the real HF artifacts by `scripts/pin-model-hashes.sh`.
pub static MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny",
        hf_file: "ggml-tiny.bin",
        size_bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        role: "CI + tests",
        encoder: Some(EncoderPackage {
            zip_file: "ggml-tiny-encoder.mlmodelc.zip",
            size_bytes: 15_037_446,
            sha256: "c88cbd2648e1f5415092bcf5256add463a0f19943e6938f46e8d4ffdebd47739",
        }),
    },
    ModelInfo {
        id: "base",
        hf_file: "ggml-base.bin",
        size_bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        role: "optional/legacy",
        encoder: Some(EncoderPackage {
            zip_file: "ggml-base-encoder.mlmodelc.zip",
            size_bytes: 37_922_638,
            sha256: "7e6ab77041942572f239b5b602f8aaa1c3ed29d73e3d8f20abea03a773541089",
        }),
    },
    ModelInfo {
        id: "small",
        hf_file: "ggml-small.bin",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        role: "fast",
        encoder: Some(EncoderPackage {
            zip_file: "ggml-small-encoder.mlmodelc.zip",
            size_bytes: 163_083_239,
            sha256: "de43fb9fed471e95c19e60ae67575c2bf09e8fb607016da171b06ddad313988b",
        }),
    },
    ModelInfo {
        id: "turbo-q5-0",
        hf_file: "ggml-large-v3-turbo-q5_0.bin",
        size_bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        role: "default (macOS)",
        encoder: Some(LARGE_V3_TURBO_ENCODER),
    },
    ModelInfo {
        id: "turbo-q8-0",
        hf_file: "ggml-large-v3-turbo-q8_0.bin",
        size_bytes: 874_188_075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
        role: "high quality",
        encoder: Some(LARGE_V3_TURBO_ENCODER),
    },
    ModelInfo {
        id: "turbo",
        hf_file: "ggml-large-v3-turbo.bin",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        role: "accuracy",
        encoder: Some(LARGE_V3_TURBO_ENCODER),
    },
];

/// Files a whisper.cpp `.mlmodelc` encoder directory must contain (checked after
/// extraction so a truncated download can never masquerade as an installed encoder).
pub const ENCODER_REQUIRED_FILES: &[&str] = &[
    "coremldata.bin",
    "metadata.json",
    "model.mil",
    "weights/weight.bin",
];

// ---------------------------------------------------------------------------
// M10A: the Parakeet fast engine (sherpa-onnx / NeMo TDT 0.6B v3 int8)
// ---------------------------------------------------------------------------

/// One pinned file of a Parakeet model directory.
///
/// Parakeet models are a *directory* of ONNX artifacts (encoder/decoder/joiner +
/// tokens), not a single ggml file, so each file carries its own size + sha256 pin.
#[derive(Debug, Clone, Copy)]
pub struct ParakeetFile {
    /// File name inside the model directory (and in the upstream repository).
    pub name: &'static str,
    /// Size in bytes (pinned alongside the sha256).
    pub size_bytes: u64,
    /// sha256 of the file.
    pub sha256: &'static str,
}

/// A Parakeet (sherpa-onnx offline transducer) model: a directory of pinned
/// artifacts under the models dir.
#[derive(Debug, Clone, Copy)]
pub struct ParakeetModelInfo {
    /// Manifest id used by `models download|remove` and the settings toggle.
    pub id: &'static str,
    /// Directory name under the models dir (the upstream release layout name).
    pub dir_name: &'static str,
    /// HuggingFace repository + revision base URL (`…/resolve/main/`).
    pub hf_base_url: &'static str,
    /// Mirror base URL selected via `SURAMAK_HF_MIRROR` (same paths).
    pub mirror_base_url: &'static str,
    /// The pinned files (all of them are required for an install).
    pub files: &'static [ParakeetFile],
    /// Primary language subtags the model covers (upstream: 25 European languages).
    /// **Indonesian is deliberately absent** — the routing invariant lives in
    /// `suramak-asr::engine` and is tested there.
    pub languages: &'static [&'static str],
    pub role: &'static str,
}

/// HuggingFace base URL for the sherpa-onnx Parakeet TDT v3 int8 export (the
/// `csukuangfj` mirror of the k2-fsa release artifacts, which ships the *same*
/// files individually — pinned per file below).
pub const PARAKEET_HF_BASE_URL: &str =
    "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/";

/// Mirror base URL selected via `SURAMAK_HF_MIRROR` (same paths).
pub const PARAKEET_HF_MIRROR_BASE_URL: &str =
    "https://hf-mirror.com/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/";

/// The one Parakeet model (M10A). Sizes + sha256 taken from the real downloaded
/// artifacts (see `docs/MODEL-PINS.md` and `scripts/pin-model-hashes.sh`).
pub const PARAKEET: ParakeetModelInfo = ParakeetModelInfo {
    id: "parakeet",
    dir_name: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    hf_base_url: PARAKEET_HF_BASE_URL,
    mirror_base_url: PARAKEET_HF_MIRROR_BASE_URL,
    files: &[
        ParakeetFile {
            name: "encoder.int8.onnx",
            size_bytes: 652_184_281,
            sha256: "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247",
        },
        ParakeetFile {
            name: "decoder.int8.onnx",
            size_bytes: 11_845_275,
            sha256: "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e",
        },
        ParakeetFile {
            name: "joiner.int8.onnx",
            size_bytes: 6_355_277,
            sha256: "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3",
        },
        ParakeetFile {
            name: "tokens.txt",
            size_bytes: 93_939,
            sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
        },
    ],
    languages: &[
        "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt",
        "mt", "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
    ],
    role: "fast engine (experimental)",
};

/// Every file an installed Parakeet directory must contain (layout check).
pub const PARAKEET_REQUIRED_FILES: &[&str] = &[
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
];

/// Looks up the Parakeet model by manifest id (`parakeet`).
pub fn find_parakeet(id: &str) -> Option<&'static ParakeetModelInfo> {
    if id == PARAKEET.id {
        Some(&PARAKEET)
    } else {
        None
    }
}

/// The Parakeet model directory under `models_dir`.
pub fn parakeet_dir(models_dir: &Path) -> PathBuf {
    models_dir.join(PARAKEET.dir_name)
}

/// Whether the Parakeet model directory is installed with every required file.
pub fn parakeet_installed(models_dir: &Path) -> bool {
    crate::download::dir_ready(&parakeet_dir(models_dir), PARAKEET_REQUIRED_FILES)
}

/// Download URL for one Parakeet file, respecting `SURAMAK_HF_MIRROR`.
pub fn parakeet_file_url(info: &ParakeetModelInfo, name: &str) -> String {
    let base = if std::env::var("SURAMAK_HF_MIRROR").is_ok() {
        info.mirror_base_url
    } else {
        info.hf_base_url
    };
    format!("{base}{name}")
}

/// Whether the Parakeet model covers `code` (a primary language subtag like `en`
/// or a regional code like `pt-BR`).
///
/// Normalizes to the primary subtag, lowercased. **Indonesian (`id`) is always
/// `false`** — the hard guard behind "Indonesian never routes to Parakeet"
/// (docs/ENGINES.md); the list itself must also never contain `id`.
pub fn parakeet_covers(code: &str) -> bool {
    let primary = code
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if primary == "id" {
        return false;
    }
    PARAKEET.languages.contains(&primary.as_str())
}

/// Looks up a model by id.
pub fn find_model(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

/// Base URL for model/encoder downloads, respecting `SURAMAK_HF_MIRROR`.
fn hf_base() -> &'static str {
    if std::env::var("SURAMAK_HF_MIRROR").is_ok() {
        HF_MIRROR_BASE_URL
    } else {
        HF_BASE_URL
    }
}

/// Returns the download URL for a model, respecting `SURAMAK_HF_MIRROR`.
pub fn model_url(info: &ModelInfo) -> String {
    format!("{}{}", hf_base(), info.hf_file)
}

/// Returns the download URL for an encoder package, respecting `SURAMAK_HF_MIRROR`.
pub fn encoder_url(package: &EncoderPackage) -> String {
    format!("{}{}", hf_base(), package.zip_file)
}

/// Whisper.cpp's Core ML encoder directory name for a model file name.
///
/// Mirrors `whisper_get_coreml_path_encoder()` in whisper.cpp (src/whisper.cpp):
/// drop the extension, drop a trailing `-qX_X` quantization suffix, then append
/// `-encoder.mlmodelc`. That is the exact directory whisper.cpp looks for next to the
/// model file, e.g. `ggml-large-v3-turbo-q5_0.bin` → `ggml-large-v3-turbo-encoder.mlmodelc`
/// (which is why all three turbo decoders share one encoder package).
pub fn encoder_dir_name(model_file_name: &str) -> String {
    let stem = match model_file_name.rfind('.') {
        Some(pos) => &model_file_name[..pos],
        None => model_file_name,
    };
    // Match whisper.cpp's `-qx_x` suffix detection (5 bytes: '-', 'q', digit, '_', digit).
    let stem = match stem.rfind('-') {
        Some(pos) => {
            let sub = &stem[pos..];
            if sub.len() == 5 && sub.get(1..2) == Some("q") && sub.get(3..4) == Some("_") {
                &stem[..pos]
            } else {
                stem
            }
        }
        None => stem,
    };
    format!("{stem}-encoder.mlmodelc")
}

/// The Core ML encoder directory whisper.cpp will look for next to `model_path`.
pub fn encoder_dir_for(model_path: &Path) -> PathBuf {
    let name = model_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let parent = model_path.parent().unwrap_or(Path::new("."));
    parent.join(encoder_dir_name(name))
}

/// Whether the Core ML encoder directory for `model_path` is installed.
pub fn encoder_installed(model_path: &Path) -> bool {
    encoder_dir_for(model_path).is_dir()
}

/// Whether `dir` contains every file an extracted encoder package must have.
pub fn encoder_dir_ready(dir: &Path) -> bool {
    crate::download::dir_ready(dir, ENCODER_REQUIRED_FILES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoder_dir_name_matches_whisper_cpp_lookup() {
        assert_eq!(
            encoder_dir_name("ggml-tiny.bin"),
            "ggml-tiny-encoder.mlmodelc"
        );
        assert_eq!(
            encoder_dir_name("ggml-large-v3-turbo-q5_0.bin"),
            "ggml-large-v3-turbo-encoder.mlmodelc"
        );
        assert_eq!(
            encoder_dir_name("ggml-large-v3-turbo-q8_0.bin"),
            "ggml-large-v3-turbo-encoder.mlmodelc"
        );
        // Unquantized and extension-less names keep their stem.
        assert_eq!(
            encoder_dir_name("ggml-large-v3-turbo.bin"),
            "ggml-large-v3-turbo-encoder.mlmodelc"
        );
        assert_eq!(encoder_dir_name("custom"), "custom-encoder.mlmodelc");
        // A `-qX_X`-looking tail that is not exactly 5 bytes is not stripped.
        assert_eq!(
            encoder_dir_name("ggml-foo-q5_0x.bin"),
            "ggml-foo-q5_0x-encoder.mlmodelc"
        );
    }

    #[test]
    fn encoder_dir_for_is_next_to_the_model_file() {
        assert_eq!(
            encoder_dir_for(Path::new("/models/ggml-tiny.bin")),
            PathBuf::from("/models/ggml-tiny-encoder.mlmodelc")
        );
        assert_eq!(
            encoder_dir_for(Path::new("ggml-large-v3-turbo-q5_0.bin")),
            PathBuf::from("ggml-large-v3-turbo-encoder.mlmodelc")
        );
    }

    #[test]
    fn every_manifest_model_pins_a_full_encoder_package() {
        for m in MODELS {
            let enc = m
                .encoder
                .unwrap_or_else(|| panic!("model {} has no encoder entry", m.id));
            assert!(
                enc.zip_file.ends_with("-encoder.mlmodelc.zip"),
                "unexpected encoder file name {}",
                enc.zip_file
            );
            assert!(enc.size_bytes > 0);
            assert_eq!(
                enc.sha256.len(),
                64,
                "sha256 must be hex-64: {}",
                enc.sha256
            );
            assert!(
                enc.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "sha256 must be hex: {}",
                enc.sha256
            );
        }
    }

    #[test]
    fn turbo_variants_share_one_encoder_package() {
        let q5 = find_model("turbo-q5-0").unwrap().encoder.unwrap();
        let q8 = find_model("turbo-q8-0").unwrap().encoder.unwrap();
        let full = find_model("turbo").unwrap().encoder.unwrap();
        assert_eq!(q5.zip_file, q8.zip_file);
        assert_eq!(q5.zip_file, full.zip_file);
        // …and all three resolve to the directory whisper.cpp expects.
        for id in ["turbo-q5-0", "turbo-q8-0", "turbo"] {
            let info = find_model(id).unwrap();
            assert_eq!(
                encoder_dir_name(info.hf_file),
                "ggml-large-v3-turbo-encoder.mlmodelc"
            );
        }
    }

    #[test]
    fn encoder_url_uses_the_hf_base() {
        let enc = find_model("tiny").unwrap().encoder.unwrap();
        assert_eq!(
            encoder_url(&enc),
            format!("{HF_BASE_URL}ggml-tiny-encoder.mlmodelc.zip")
        );
    }

    // ---- M10A: the Parakeet manifest ---------------------------------------

    #[test]
    fn parakeet_pins_are_well_formed_and_cover_the_layout() {
        assert_eq!(PARAKEET.id, "parakeet");
        assert_eq!(
            PARAKEET.dir_name,
            "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8"
        );
        assert!(PARAKEET.role.contains("fast"), "role: {}", PARAKEET.role);
        // Every required file is a pinned file (the layout check cannot drift
        // from the download set).
        let pinned: Vec<&str> = PARAKEET.files.iter().map(|f| f.name).collect();
        for required in PARAKEET_REQUIRED_FILES {
            assert!(
                pinned.contains(required),
                "{required} is required but not pinned"
            );
        }
        assert_eq!(PARAKEET.files.len(), PARAKEET_REQUIRED_FILES.len());
        for file in PARAKEET.files {
            assert!(file.size_bytes > 0, "{}: size", file.name);
            assert_eq!(file.sha256.len(), 64, "{}: sha256", file.name);
            assert!(
                file.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{}: sha256 must be hex",
                file.name
            );
        }
    }

    #[test]
    fn parakeet_urls_use_the_hf_base_and_carry_every_pinned_file() {
        for file in PARAKEET.files {
            assert_eq!(
                parakeet_file_url(&PARAKEET, file.name),
                format!("{PARAKEET_HF_BASE_URL}{}", file.name)
            );
        }
        assert_eq!(
            parakeet_file_url(&PARAKEET, "encoder.int8.onnx"),
            "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/\
             resolve/main/encoder.int8.onnx"
        );
    }

    #[test]
    fn parakeet_coverage_has_25_languages_and_never_indonesian() {
        // Upstream: 25 European languages (no Indonesian — the M10A invariant).
        assert_eq!(PARAKEET.languages.len(), 25);
        assert!(
            !PARAKEET.languages.contains(&"id"),
            "the Parakeet coverage list must never contain `id`"
        );
        for covered in ["en", "de", "fr", "es", "pt", "uk", "bg"] {
            assert!(parakeet_covers(covered), "{covered} must be covered");
        }
        // The hard guard: `id` is false no matter what the list says.
        assert!(!parakeet_covers("id"));
        assert!(!parakeet_covers("ID"));
        assert!(!parakeet_covers("id-ID"));
        // Regional codes resolve to their primary subtag.
        assert!(parakeet_covers("pt-BR"));
        assert!(parakeet_covers("en-US"));
        assert!(parakeet_covers(" EN "));
        // Uncovered languages (and empty input) are false.
        for uncovered in ["", "ja", "ko", "zh", "ar", "jv", "xx"] {
            assert!(
                !parakeet_covers(uncovered),
                "{uncovered:?} must not be covered"
            );
        }
    }

    #[test]
    fn parakeet_install_detection_requires_every_file() {
        let dir = std::env::temp_dir().join(format!("suramak-parakeet-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert!(!parakeet_installed(&dir), "nothing installed yet");
        let model_dir = parakeet_dir(&dir);
        std::fs::create_dir_all(&model_dir).unwrap();
        for (i, file) in PARAKEET.files.iter().enumerate() {
            std::fs::write(model_dir.join(file.name), b"x").unwrap();
            let complete = i + 1 == PARAKEET.files.len();
            assert_eq!(
                parakeet_installed(&dir),
                complete,
                "with {} of {} files the install must be {}",
                i + 1,
                PARAKEET.files.len(),
                if complete { "detected" } else { "incomplete" }
            );
        }
        // A directory named like the model file is not an install (dir_ready
        // checks files, so a stray directory cannot masquerade).
        std::fs::remove_dir_all(&model_dir).unwrap();
        std::fs::create_dir_all(model_dir.join("tokens.txt")).unwrap();
        for file in PARAKEET.files.iter().filter(|f| f.name != "tokens.txt") {
            std::fs::write(model_dir.join(file.name), b"x").unwrap();
        }
        assert!(!parakeet_installed(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn find_parakeet_accepts_only_the_manifest_id() {
        assert_eq!(find_parakeet("parakeet").unwrap().id, "parakeet");
        for unknown in ["tiny", "parakeet-tdt-0.6b-v3", ""] {
            assert!(find_parakeet(unknown).is_none(), "{unknown:?}");
        }
    }
}
