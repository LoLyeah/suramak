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
}
