//! `prompt-budget` — measure a model's Whisper prompt budget and tokenize text
//! with the model's own tokenizer (M15.5).
//!
//! The settings surface measures the same way, on a background thread, and
//! caches the result in the library (`prompt_budgets`). This command is the
//! headless form: it opens the model read-only, reads `n_text_ctx`, reports the
//! cap whisper.cpp allows the initial prompt (`n_text_ctx / 2`) and, when asked,
//! counts tokens for `--text` and/or builds the prompt for a glossary file.
//!
//! Nothing here transcribes: no `FullParams`, no `full()`, no state.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Args;
use suramak_asr::prompt_budget::ModelTokenizer;
use suramak_core::textrules::Glossary;

#[derive(Args)]
pub struct PromptBudgetArgs {
    /// Model id (`tiny`, `large-v3`, …) or a path to a ggml `.bin` file.
    #[arg(long, default_value = "tiny")]
    pub model: String,
    /// Text to tokenize with the model's own tokenizer (repeatable).
    #[arg(long = "text")]
    pub text: Vec<String>,
    /// A file with one glossary term per line; the command prints the plan the
    /// transcription path would send.
    #[arg(long)]
    pub glossary: Option<PathBuf>,
}

pub fn run(args: PromptBudgetArgs) -> Result<()> {
    let path = crate::cmd::resolve_model(&args.model)?;
    let model_id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&args.model)
        .to_string();
    let tokenizer = ModelTokenizer::load(&path, model_id)
        .with_context(|| format!("cannot measure {}", path.display()))?;
    let measurement = tokenizer.measurement();

    println!("suramak prompt-budget");
    println!("  model:       {}", path.display());
    println!("  model id:    {}", measurement.model_id);
    println!(
        "  n_text_ctx:  {} tokens (measured from the loaded model)",
        measurement.n_text_ctx
    );
    println!(
        "  prompt cap:  {} tokens (n_text_ctx / 2 — whisper.cpp keeps half for the initial prompt)",
        measurement.cap_tokens
    );
    println!("  measured at: {}", measurement.measured_at.to_rfc3339());

    for text in &args.text {
        println!("  tokens:      {}  {text:?}", tokenizer.token_count(text));
    }

    if let Some(glossary_path) = &args.glossary {
        let raw = std::fs::read_to_string(glossary_path)
            .with_context(|| format!("cannot read {}", glossary_path.display()))?;
        let glossary = Glossary::parse(&raw);
        let plan = tokenizer.plan(None, &glossary);
        println!();
        println!(
            "  glossary:    {} terms ({} refused, {} over the cap)",
            glossary.len(),
            glossary.rejected().len(),
            glossary.over_cap()
        );
        println!(
            "  plan:        {} of {} tokens, {} terms in, {} dropped",
            plan.used_tokens,
            plan.cap_tokens,
            plan.included.len(),
            plan.dropped.len()
        );
        if let Some(feedback) = plan.feedback() {
            println!("  note:        {feedback}");
        }
        if let Some(prompt) = plan.prompt() {
            println!();
            println!("  prompt: {prompt}");
        }
    }

    Ok(())
}
