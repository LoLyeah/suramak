//! M15.5 — the prompt budget **measured** from a real model.
//!
//! The glossary builder in `suramak_core::textrules` is pure: it needs a token
//! cap and a token counter. This module supplies both from whisper.cpp itself:
//!
//! - the cap is `n_text_ctx / 2` read off the loaded model
//!   ([`whisper_rs::WhisperContext::n_text_ctx`]) — the half whisper.cpp keeps
//!   for `<|startofprev|>` prompt tokens and silently truncates beyond;
//! - the counter is the model's own tokenizer
//!   ([`whisper_rs::WhisperContext::tokenize`]) — a real byte-level BPE count,
//!   not a character estimate.
//!
//! Nothing here touches transcription: no `FullParams`, no `full()`, no state
//! creation. A measurement opens the model read-only on the CPU, tokenizes the
//! strings the caller hands it, and drops the context. It is called when the
//! settings surface needs a budget for a model that has no cached measurement
//! yet, and the result is cached by the caller (`Store::record_prompt_measurement`).
//!
//! Loading a model costs a few hundred milliseconds (tiny) to seconds
//! (large-v3) and its RAM for as long as the measurement lives, so the app
//! measures **once per model** and reads the cache afterwards. When no
//! measurement exists, the core builder's documented fallback (224) is used and
//! its caption says so.
//!
//! ## Why not measure inside the transcription job
//!
//! The worker already holds a context, so reading `n_text_ctx` there would be
//! free — but that would put prompt-budget bookkeeping on the inference path and
//! change whisper-rs call sites this slice was told to leave untouched. The
//! measurement path is deliberately separate and read-only.

use std::path::{Path, PathBuf};

use suramak_core::textrules::{
    build_prompt, EstimatedTokens, Glossary, PromptBudget, PromptMeasurement, PromptPlan,
    TokenCounter,
};
use whisper_rs::{WhisperContext, WhisperContextParameters};

use crate::whisper::AsrError;

/// A loaded model used only to measure tokens and read the prompt cap.
///
/// Owns its context: drop it and the memory goes back. Never used to decode.
pub struct ModelTokenizer {
    context: WhisperContext,
    model_id: String,
    n_text_ctx: i32,
}

impl std::fmt::Debug for ModelTokenizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelTokenizer")
            .field("model_id", &self.model_id)
            .field("n_text_ctx", &self.n_text_ctx)
            .finish_non_exhaustive()
    }
}

impl ModelTokenizer {
    /// Opens `model_path` (CPU, no GPU allocation) and reads its text context.
    pub fn load(model_path: &Path, model_id: impl Into<String>) -> Result<Self, AsrError> {
        if !model_path.exists() {
            return Err(AsrError::ModelFileMissing(model_path.to_path_buf()));
        }
        let mut params = WhisperContextParameters::default();
        // Tokenization never runs a graph; a CPU context is enough and avoids
        // touching the GPU backends in a settings-time measurement.
        params.use_gpu(false);
        let context = WhisperContext::new_with_params(model_path, params).map_err(|err| {
            AsrError::ModelLoad {
                path: model_path.to_path_buf(),
                message: err.to_string(),
            }
        })?;
        let n_text_ctx = context.n_text_ctx();
        if n_text_ctx <= 0 {
            return Err(AsrError::Inference(format!(
                "model {} reports a non-positive text context ({n_text_ctx})",
                model_path.display()
            )));
        }
        Ok(Self {
            context,
            model_id: model_id.into(),
            n_text_ctx,
        })
    }

    /// The model's text-context size in tokens (448 on the multilingual models).
    pub fn n_text_ctx(&self) -> i32 {
        self.n_text_ctx
    }

    /// The measurement to cache: model id, context size and the resulting cap.
    pub fn measurement(&self) -> PromptMeasurement {
        PromptMeasurement::new(self.model_id.clone(), self.n_text_ctx)
    }

    /// The prompt budget this model implies.
    pub fn budget(&self) -> PromptBudget {
        PromptBudget::measured(self.model_id.clone(), self.n_text_ctx)
    }

    /// The model's own token count for `text`.
    ///
    /// A tokenizer failure (invalid text) falls back to the conservative
    /// estimate rather than failing the settings surface: over-counting can
    /// only leave terms out, which the plan reports.
    pub fn token_count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        let limit = self.n_text_ctx.max(1) as usize;
        match self.context.tokenize(text, limit) {
            Ok(tokens) => tokens.len(),
            Err(_) => EstimatedTokens.count(text),
        }
    }

    /// Builds the prompt this model would receive for `glossary`, within its
    /// measured budget. The returned plan carries the truncation report.
    pub fn plan(&self, base: Option<&str>, glossary: &Glossary) -> PromptPlan {
        build_prompt(base, glossary, &self.budget(), self)
    }
}

impl TokenCounter for ModelTokenizer {
    fn count(&self, text: &str) -> usize {
        self.token_count(text)
    }
}

/// Opens `model_path`, measures it and drops the context; the measurement is
/// what a caller caches.
pub fn measure_model(
    model_path: &Path,
    model_id: impl Into<String>,
) -> Result<PromptMeasurement, AsrError> {
    Ok(ModelTokenizer::load(model_path, model_id)?.measurement())
}

/// The token count `model_path`'s tokenizer gives `text` (one-shot; prefer
/// [`ModelTokenizer`] when several strings are measured).
pub fn count_tokens(
    model_path: &Path,
    model_id: impl Into<String>,
    text: &str,
) -> Result<usize, AsrError> {
    Ok(ModelTokenizer::load(model_path, model_id)?.token_count(text))
}

/// Where the app keeps its models — the natural place for a measurement to look
/// when the caller only knows a model id.
pub fn model_path_for(model_id: &str) -> Option<PathBuf> {
    let dir = suramak_core::paths::models_dir().ok()?;
    let name = if model_id.ends_with(".bin") {
        model_id.to_string()
    } else {
        format!("{model_id}.bin")
    };
    let path = dir.join(name);
    path.exists().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The model a developer machine may have; the test skips without it (no
    /// network, no fixture download in the workspace gate).
    fn local_model() -> Option<PathBuf> {
        let path = suramak_core::paths::models_dir()
            .ok()?
            .join("ggml-tiny.bin");
        path.exists().then_some(path)
    }

    #[test]
    fn a_real_model_reports_its_context_and_tokenizes() {
        let Some(path) = local_model() else {
            eprintln!("skipping: no ggml-tiny.bin under the models directory");
            return;
        };
        let tokenizer = ModelTokenizer::load(&path, "ggml-tiny").expect("load tiny");
        let n_text_ctx = tokenizer.n_text_ctx();
        assert!(
            n_text_ctx > 0,
            "whisper reports a positive text context: {n_text_ctx}"
        );
        // The cap is measured, not assumed: half the model's own context.
        let measurement = tokenizer.measurement();
        assert_eq!(measurement.cap_tokens, n_text_ctx as usize / 2);
        assert_eq!(tokenizer.budget().cap_tokens(), n_text_ctx as usize / 2);

        // The tokenizer is a real BPE count: more characters, more tokens, and
        // a short Indonesian phrase is a handful of tokens.
        let short = tokenizer.token_count("halo");
        let long = tokenizer.token_count("Kecamatan Cilandak, Jakarta Selatan");
        assert!((1..=4).contains(&short), "halo = {short} tokens");
        assert!(long > short, "{long} must exceed {short}");
        assert!(long < 40, "a 38-character phrase is not 40 tokens: {long}");
        assert_eq!(tokenizer.token_count(""), 0);
    }

    #[test]
    fn the_measured_budget_bounds_the_real_prompt() {
        let Some(path) = local_model() else {
            eprintln!("skipping: no ggml-tiny.bin under the models directory");
            return;
        };
        let tokenizer = ModelTokenizer::load(&path, "ggml-tiny").expect("load tiny");
        // A glossary far larger than the budget: the plan must fit exactly and
        // report what it dropped, measured with the model's own tokenizer.
        let terms: String = (0..64)
            .map(|index| format!("Kecamatan Contoh Nomor {index}\n"))
            .collect();
        let glossary = Glossary::parse(&terms);
        let plan = tokenizer.plan(None, &glossary);
        assert!(plan.used_tokens <= plan.cap_tokens, "{plan:?}");
        assert_eq!(plan.used_tokens, tokenizer.token_count(&plan.text));
        assert_eq!(plan.cap_tokens, tokenizer.n_text_ctx() as usize / 2);
        assert!(
            !plan.dropped.is_empty(),
            "64 four-word terms cannot fit 224 tokens"
        );
        assert_eq!(plan.included.len() + plan.dropped.len(), 64);
        assert!(plan.feedback().is_some());
    }

    #[test]
    fn a_missing_model_is_a_named_error_not_a_panic() {
        let err = ModelTokenizer::load(Path::new("/nonexistent/ggml-none.bin"), "none")
            .expect_err("missing model");
        assert!(matches!(err, AsrError::ModelFileMissing(_)), "{err}");
    }
}
