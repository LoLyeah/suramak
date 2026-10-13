/// A model available for download from HuggingFace.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: &'static str,
    pub hf_file: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    pub role: &'static str,
}

/// HuggingFace base URL for the whisper.cpp model repository.
pub const HF_BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

/// Mirror base URL selected via `SURAMAK_HF_MIRROR` env var.
pub const HF_MIRROR_BASE_URL: &str = "https://hf-mirror.com/ggerganov/whisper.cpp/resolve/main/";

/// All supported models. Values pasted verbatim from docs/MODEL-PINS.md.
pub static MODELS: &[ModelInfo] = &[
    ModelInfo {
        id: "tiny",
        hf_file: "ggml-tiny.bin",
        size_bytes: 77_691_713,
        sha256: "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21",
        role: "CI + tests",
    },
    ModelInfo {
        id: "base",
        hf_file: "ggml-base.bin",
        size_bytes: 147_951_465,
        sha256: "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe",
        role: "optional/legacy",
    },
    ModelInfo {
        id: "small",
        hf_file: "ggml-small.bin",
        size_bytes: 487_601_967,
        sha256: "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b",
        role: "fast",
    },
    ModelInfo {
        id: "turbo-q5-0",
        hf_file: "ggml-large-v3-turbo-q5_0.bin",
        size_bytes: 574_041_195,
        sha256: "394221709cd5ad1f40c46e6031ca61bce88931e6e088c188294c6d5a55ffa7e2",
        role: "default (macOS)",
    },
    ModelInfo {
        id: "turbo-q8-0",
        hf_file: "ggml-large-v3-turbo-q8_0.bin",
        size_bytes: 874_188_075,
        sha256: "317eb69c11673c9de1e1f0d459b253999804ec71ac4c23c17ecf5fbe24e259a1",
        role: "high quality",
    },
    ModelInfo {
        id: "turbo",
        hf_file: "ggml-large-v3-turbo.bin",
        size_bytes: 1_624_555_275,
        sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
        role: "accuracy",
    },
];

/// Looks up a model by id.
pub fn find_model(id: &str) -> Option<&'static ModelInfo> {
    MODELS.iter().find(|m| m.id == id)
}

/// Returns the download URL for a model, respecting `SURAMAK_HF_MIRROR`.
pub fn model_url(info: &ModelInfo) -> String {
    let base = std::env::var("SURAMAK_HF_MIRROR")
        .ok()
        .map(|_| HF_MIRROR_BASE_URL)
        .unwrap_or(HF_BASE_URL);
    format!("{}{}", base, info.hf_file)
}
