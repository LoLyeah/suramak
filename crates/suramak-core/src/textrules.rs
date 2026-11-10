//! M15.5 — the bounded glossary, the post-correction dictionary, the opt-in
//! Indonesian normalization rules and the clean-export preview.
//!
//! Four pieces with one rule between them: **nothing here ever writes the
//! inference text**. `source_text` (M15-A) is immutable; every transform in this
//! module produces a *derived* string (or a derived copy of a transcript) that
//! can be recomputed from the stored text and the stored rules at any time.
//!
//! ## 1. Glossary → Whisper initial prompt ([`Glossary`], [`PromptBudget`])
//!
//! Whisper's initial prompt is not free: whisper.cpp keeps at most half of the
//! model's text context for the prompt (`n_text_ctx / 2`, 224 tokens on the
//! 448-token models), and it silently drops what does not fit. So the builder
//!
//! - takes the cap from a **measurement** ([`PromptBudget::measured`]) or from
//!   the documented conservative default ([`PromptBudget::unmeasured`]) when no
//!   measurement exists yet — never from a number invented here;
//! - counts tokens with the caller's [`TokenCounter`] (the app supplies the
//!   model's real tokenizer; tests supply exact counters and the built-in
//!   estimate);
//! - adds terms in stored order, skipping a term that does not fit and
//!   continuing with the next (deterministic, and a long term cannot shut out
//!   every short one after it);
//! - reports every dropped term, so the settings surface can say how many were
//!   left out and why.
//!
//! Terms are joined with `", "` and nothing else: no "Glossary:" label and no
//! sentence wrapper, because anthropomorphic instruction text is what tends to
//! leak into the transcript. This is prompt biasing, not a WER guarantee.
//!
//! ## 2. Post-correction dictionary ([`Dictionary`])
//!
//! Whole-word, case-sensitive, single-pass, longest-match-first replacements
//! applied after inference. Two algebraic properties make it safe to re-run:
//!
//! - **idempotent** — one pass never re-scans its own output, and entries whose
//!   `from` would appear inside another entry's `to` are rejected at parse time
//!   (no chains), so `apply(apply(x)) == apply(x)` for every input;
//! - **reversible** — [`Corrected`] carries an edit log with the original text
//!   of every replaced span, so [`Corrected::restore`] rebuilds the exact input
//!   from the corrected text alone.
//!
//! ## 3. Indonesian normalization ([`Normalizer`])
//!
//! Conservative, opt-in, word-boundary-only rewrites of colloquial Indonesian
//! tokens (`gak` → `tidak`, `kalo` → `kalau`, …). Code-switching is respected
//! by three guards, checked per token:
//!
//! - a rule never fires inside a longer token (word boundaries throughout);
//! - a rule with [`Guard::SafeContext`] never fires next to a determiner
//!   (`the/a/an/this/that/ini/itu/…` possessives) or at a clause-final position
//!   (immediately before `. , ? ! ; :` or the end of the text) — the `kayak`
//!   fixture: the English boat noun and the Indonesian trailing hedge both stay;
//! - no rule fires in a window (±3 tokens) that reads as English (two or more
//!   English function words), so an English run inside an Indonesian sentence is
//!   left alone.
//!
//! This is a heuristic over a fixed rule table, not a POS tagger. It is opt-in,
//! previewed, and every application is reversible through its edit log.
//!
//! ## 4. Clean export ([`preview`], [`ExportPreview`])
//!
//! [`preview`] computes the cleaned rows and their diff; it has no I/O and takes
//! `&Transcript`. Only [`ExportPreview::apply`] builds the derived transcript the
//! exporter writes, and only the view's explicit Apply action calls it. An empty
//! preview (nothing changed) still requires Apply — the toggle never exports by
//! itself.

use serde::{Deserialize, Serialize};

use crate::transcript::Transcript;

/// The default number of glossary terms kept. A bound on prompt growth and on
/// the settings field's size; terms are added in order and the rest are
/// reported as dropped by the cap.
pub const DEFAULT_GLOSSARY_CAP: usize = 64;

/// The longest single term accepted (characters, trimmed). Longer "terms" are
/// sentences, and a sentence in a comma list biases decoding away from the
/// audio.
pub const MAX_TERM_CHARS: usize = 48;

/// The documented fallback prompt budget, used **only** until a real model
/// measurement exists: 448 text-context tokens ÷ 2. [`PromptBudget::measured`]
/// replaces it with the model's own numbers.
pub const FALLBACK_PROMPT_CAP_TOKENS: usize = 224;

// ---------------------------------------------------------------------------
// 1. Glossary + prompt budget
// ---------------------------------------------------------------------------

/// A bounded, de-duplicated list of prompt terms in stored order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Glossary {
    terms: Vec<String>,
    /// Entries the parser refused (too long, multi-line after unfolding, …).
    rejected: Vec<String>,
    /// Entries dropped by [`DEFAULT_GLOSSARY_CAP`].
    over_cap: usize,
}

impl Glossary {
    /// Parses the settings field: one term per line, trimmed, blanks ignored.
    ///
    /// A line longer than [`MAX_TERM_CHARS`] is rejected (and counted), not
    /// truncated: truncating a name produces a different name. Duplicates are
    /// folded case-insensitively, keeping the first spelling.
    pub fn parse(text: &str) -> Self {
        let mut terms: Vec<String> = Vec::new();
        let mut rejected = Vec::new();
        let mut over_cap = 0usize;
        for raw in text.lines() {
            let term = raw.trim();
            if term.is_empty() {
                continue;
            }
            let term = collapse_spaces(term);
            if term.chars().count() > MAX_TERM_CHARS {
                rejected.push(term);
                continue;
            }
            if terms
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&term))
            {
                continue;
            }
            if terms.len() == DEFAULT_GLOSSARY_CAP {
                over_cap += 1;
                continue;
            }
            terms.push(term);
        }
        Self {
            terms,
            rejected,
            over_cap,
        }
    }

    /// The stored terms, in order.
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    pub fn len(&self) -> usize {
        self.terms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Lines the parser refused, for the settings caption.
    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }

    /// How many otherwise-valid terms the 64-term cap dropped.
    pub fn over_cap(&self) -> usize {
        self.over_cap
    }

    /// The field's text form (round-trips through [`Glossary::parse`] for every
    /// accepted term).
    pub fn to_field(&self) -> String {
        self.terms.join("\n")
    }
}

/// Where a prompt budget's cap came from. The distinction is user-visible: an
/// unmeasured budget says so instead of pretending to be exact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetSource {
    /// Measured from a loaded model: `n_text_ctx` and the model id are real.
    Measured { model_id: String, n_text_ctx: i32 },
    /// The documented 224-token fallback (448 ÷ 2), no model measured yet.
    Fallback,
}

/// The prompt's token budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptBudget {
    cap_tokens: usize,
    source: BudgetSource,
}

impl PromptBudget {
    /// The budget a **measurement** produced. whisper.cpp keeps at most half of
    /// the model's text context for the initial prompt, so the cap is
    /// `n_text_ctx / 2` with a floor of one token.
    pub fn measured(model_id: impl Into<String>, n_text_ctx: i32) -> Self {
        let cap = (n_text_ctx.max(2) / 2) as usize;
        Self {
            cap_tokens: cap.max(1),
            source: BudgetSource::Measured {
                model_id: model_id.into(),
                n_text_ctx,
            },
        }
    }

    /// The documented fallback, for when no model has been measured yet.
    pub fn unmeasured() -> Self {
        Self {
            cap_tokens: FALLBACK_PROMPT_CAP_TOKENS,
            source: BudgetSource::Fallback,
        }
    }

    pub fn cap_tokens(&self) -> usize {
        self.cap_tokens
    }

    pub fn source(&self) -> &BudgetSource {
        &self.source
    }

    /// The caption the settings surface shows next to the glossary field.
    pub fn caption(&self) -> String {
        match &self.source {
            BudgetSource::Measured {
                model_id,
                n_text_ctx,
            } => format!(
                "prompt budget: {} tokens measured from {model_id} (n_text_ctx {n_text_ctx} ÷ 2)",
                self.cap_tokens
            ),
            BudgetSource::Fallback => format!(
                "prompt budget: {} tokens (documented fallback — no model measured yet)",
                self.cap_tokens
            ),
        }
    }
}

/// Counts the tokens a model would see. The app passes the model's real
/// tokenizer ([`crate::textrules::EstimatedTokens`] is the dependency-free
/// fallback the tests and the pre-measurement path use).
pub trait TokenCounter {
    fn count(&self, text: &str) -> usize;
}

/// A deliberately conservative estimate: `ceil(bytes / 3)`.
///
/// Whisper's byte-level BPE averages ~4 bytes per token for English and fewer
/// for Indonesian subwords, so dividing by 3 over-counts — the builder then
/// leaves terms out rather than sending a prompt the model would truncate.
#[derive(Debug, Clone, Copy, Default)]
pub struct EstimatedTokens;

impl TokenCounter for EstimatedTokens {
    fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            0
        } else {
            text.len().div_ceil(3)
        }
    }
}

impl<F> TokenCounter for F
where
    F: Fn(&str) -> usize,
{
    fn count(&self, text: &str) -> usize {
        self(text)
    }
}

/// What the builder decided: the prompt text, what fitted, and what did not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptPlan {
    pub text: String,
    pub used_tokens: usize,
    pub cap_tokens: usize,
    /// Terms that made it into the prompt, in order.
    pub included: Vec<String>,
    /// Terms left out because they would have exceeded the cap.
    pub dropped: Vec<String>,
    /// The settings prompt was included.
    pub base_included: bool,
    /// The settings prompt alone already exceeds the cap (kept verbatim; every
    /// glossary term is then dropped).
    pub base_over_budget: bool,
}

impl PromptPlan {
    /// Whether anything was left out (the settings surface's truncation
    /// feedback trigger).
    pub fn truncated(&self) -> bool {
        !self.dropped.is_empty() || self.base_over_budget
    }

    /// One sentence for the settings caption; `None` when nothing was dropped.
    pub fn feedback(&self) -> Option<String> {
        if self.dropped.is_empty() && !self.base_over_budget {
            return None;
        }
        let mut message = format!(
            "{} of {} glossary terms dropped — the prompt is capped at {} tokens",
            self.dropped.len(),
            self.dropped.len() + self.included.len(),
            self.cap_tokens
        );
        if self.base_over_budget {
            message.push_str(
                "; the initial prompt alone already exceeds the cap, so no terms were added",
            );
        }
        Some(message)
    }

    /// The prompt a job should send: `None` when there is nothing to send.
    pub fn prompt(&self) -> Option<&str> {
        if self.text.is_empty() {
            None
        } else {
            Some(&self.text)
        }
    }
}

/// Builds the initial prompt from the settings prompt and the glossary, within
/// `budget`, counting with `counter`.
///
/// Deterministic: the same `(base, glossary, budget, counter)` always produces
/// the same plan. The base prompt is never rewritten; when it alone exceeds the
/// cap it is kept and every term is dropped ([`PromptPlan::base_over_budget`]).
pub fn build_prompt(
    base: Option<&str>,
    glossary: &Glossary,
    budget: &PromptBudget,
    counter: &dyn TokenCounter,
) -> PromptPlan {
    let cap = budget.cap_tokens();
    let base = base.map(str::trim).filter(|value| !value.is_empty());
    let mut text = String::new();
    let mut base_included = false;
    let base_over_budget = false;
    if let Some(base) = base {
        if counter.count(base) > cap {
            // Keep the user's wording verbatim; add nothing that would push it
            // further over the cap the model would silently truncate anyway.
            return PromptPlan {
                text: base.to_string(),
                used_tokens: counter.count(base),
                cap_tokens: cap,
                included: Vec::new(),
                dropped: glossary.terms().to_vec(),
                base_included: true,
                base_over_budget: true,
            };
        }
        text.push_str(base);
        base_included = true;
    }

    let mut included = Vec::new();
    let mut dropped = Vec::new();
    for term in glossary.terms() {
        let mut candidate = text.clone();
        if !candidate.is_empty() {
            candidate.push_str(", ");
        }
        candidate.push_str(term);
        if counter.count(&candidate) <= cap {
            text = candidate;
            included.push(term.clone());
        } else {
            dropped.push(term.clone());
        }
    }

    PromptPlan {
        used_tokens: counter.count(&text),
        text,
        cap_tokens: cap,
        included,
        dropped,
        base_included,
        base_over_budget,
    }
}

// ---------------------------------------------------------------------------
// 2. Post-correction dictionary
// ---------------------------------------------------------------------------

/// One whole-word replacement, applied after inference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Correction {
    pub from: String,
    pub to: String,
}

/// A replacement actually applied, with the output span it occupies and the
/// exact text it replaced — enough to undo it from the corrected string alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedEdit {
    /// Byte span of the replacement in the corrected text.
    pub out_start: usize,
    pub out_end: usize,
    /// The bytes this edit replaced (the original text of the span).
    pub original: String,
}

/// A stored measurement of one model's prompt budget.
///
/// Written once per model by the measurement path (the ASR crate opens the
/// model, reads `n_text_ctx` and reports it) and read back by the settings
/// surface and the transcription path. `cap_tokens` is stored beside
/// `n_text_ctx` so a reader never has to re-derive the rule, but
/// [`PromptMeasurement::budget`] recomputes it — the two agree by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptMeasurement {
    pub model_id: String,
    pub n_text_ctx: i32,
    pub cap_tokens: usize,
    pub measured_at: chrono::DateTime<chrono::Utc>,
}

impl PromptMeasurement {
    pub fn new(model_id: impl Into<String>, n_text_ctx: i32) -> Self {
        let model_id = model_id.into();
        let budget = PromptBudget::measured(model_id.clone(), n_text_ctx);
        Self {
            model_id,
            n_text_ctx,
            cap_tokens: budget.cap_tokens(),
            measured_at: chrono::Utc::now(),
        }
    }

    pub fn budget(&self) -> PromptBudget {
        PromptBudget::measured(self.model_id.clone(), self.n_text_ctx)
    }
}

/// Every stored text rule and the two opt-ins, as one value the settings form
/// edits and the store persists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSettings {
    pub glossary: Glossary,
    pub dictionary: Dictionary,
    /// Opt-in Indonesian normalization (rules applied to a derived layer only).
    pub normalize: bool,
    /// Default for the export sheet's "apply dictionary" tick.
    pub clean_dictionary: bool,
    /// Default for the export sheet's "collapse whitespace" tick.
    pub clean_whitespace: bool,
    /// The separator a segment merge joins text with (M15.3). Free-form; the
    /// UI offers the three common ones and accepts a custom string.
    pub merge_separator: String,
}

impl Default for TextSettings {
    fn default() -> Self {
        Self {
            glossary: Glossary::default(),
            dictionary: Dictionary::default(),
            normalize: false,
            clean_dictionary: false,
            clean_whitespace: false,
            merge_separator: " ".to_string(),
        }
    }
}

impl TextSettings {
    pub fn rules(&self) -> TextRules {
        TextRules {
            dictionary: self.dictionary.clone(),
            normalize: self.normalize,
        }
    }

    /// The export options this settings row seeds.
    pub fn clean_options(&self) -> CleanOptions {
        CleanOptions {
            dictionary: self.clean_dictionary,
            normalize: self.normalize,
            collapse_whitespace: self.clean_whitespace,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.glossary.is_empty() && self.dictionary.is_empty() && !self.normalize
    }
}

/// The result of applying rules to one string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Corrected {
    pub text: String,
    pub edits: Vec<AppliedEdit>,
}

impl Corrected {
    /// Rebuilds the exact input from the corrected text and the edit log.
    ///
    /// The inverse is structural (the recorded original spans), not a second
    /// rule pass, so it cannot double-correct or drift.
    pub fn restore(&self) -> String {
        let mut out = String::with_capacity(self.text.len());
        let mut cursor = 0usize;
        for edit in &self.edits {
            if edit.out_start < cursor || edit.out_end > self.text.len() {
                // Defensive: a malformed log returns the text unchanged rather
                // than corrupting it.
                return self.text.clone();
            }
            out.push_str(&self.text[cursor..edit.out_start]);
            out.push_str(&edit.original);
            cursor = edit.out_end;
        }
        out.push_str(&self.text[cursor..]);
        out
    }
}

/// Parsed dictionary entries plus the lines the parser refused.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dictionary {
    entries: Vec<Correction>,
    rejected: Vec<String>,
}

impl Dictionary {
    /// Parses `from = to` lines (`->` and `=>` also separate).
    ///
    /// Rejections, each with a reason the settings caption can name:
    /// empty sides, equal sides, a `from` longer than [`MAX_TERM_CHARS`], a
    /// duplicate `from`, and — the important one — a `from` that appears as a
    /// whole word inside another entry's `to`, which would make a second pass
    /// chain (`a→b`, `b→c`): the dictionary would stop being idempotent.
    pub fn parse(text: &str) -> Self {
        let mut entries: Vec<Correction> = Vec::new();
        let mut rejected = Vec::new();
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            let Some((from, to)) = split_pair(line) else {
                rejected.push(line.to_string());
                continue;
            };
            let (from, to) = (collapse_spaces(from), collapse_spaces(to));
            if from.is_empty() || to.is_empty() || from == to {
                rejected.push(line.to_string());
                continue;
            }
            if from.chars().count() > MAX_TERM_CHARS || to.chars().count() > MAX_TERM_CHARS {
                rejected.push(line.to_string());
                continue;
            }
            if entries
                .iter()
                .any(|entry| entry.from.eq_ignore_ascii_case(&from))
            {
                rejected.push(line.to_string());
                continue;
            }
            // No chains: a second application must be a no-op. Check both
            // directions — a new `from` inside an existing `to` would chain
            // on the *next* pass just as surely.
            if entries
                .iter()
                .any(|entry| contains_word(&to, &entry.from) || contains_word(&entry.to, &from))
            {
                rejected.push(line.to_string());
                continue;
            }
            entries.push(Correction { from, to });
        }
        // Longest `from` first, then alphabetical — deterministic matching when
        // two entries start at the same byte.
        entries.sort_by(|a, b| {
            b.from
                .len()
                .cmp(&a.from.len())
                .then_with(|| a.from.cmp(&b.from))
        });
        Self { entries, rejected }
    }

    pub fn entries(&self) -> &[Correction] {
        &self.entries
    }

    pub fn rejected(&self) -> &[String] {
        &self.rejected
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// The field's text form (round-trips for every accepted entry).
    pub fn to_field(&self) -> String {
        self.entries
            .iter()
            .map(|entry| format!("{} = {}", entry.from, entry.to))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Applies every entry in one left-to-right pass over `text`.
    ///
    /// A replacement's output is never re-scanned, so cascades are structurally
    /// impossible; combined with the parse-time no-chain check this makes the
    /// function idempotent.
    pub fn apply(&self, text: &str) -> Corrected {
        self.apply_with(text, false)
    }

    /// Applies the entries backwards (`to` → `from`) — the text-level inverse.
    ///
    /// [`Corrected::restore`] is the exact inverse; this exists for a derived
    /// string that no longer carries its edit log, and is exact whenever `to`
    /// does not occur in the text for any other reason.
    pub fn apply_inverse(&self, text: &str) -> Corrected {
        self.apply_with(text, true)
    }

    fn apply_with(&self, text: &str, inverse: bool) -> Corrected {
        let mut out = String::with_capacity(text.len());
        let mut edits = Vec::new();
        let mut cursor = 0usize;
        while cursor < text.len() {
            let matched = self.entries.iter().find_map(|entry| {
                let (needle, replacement) = if inverse {
                    (&entry.to, &entry.from)
                } else {
                    (&entry.from, &entry.to)
                };
                let rest = &text[cursor..];
                rest.strip_prefix(needle)?;
                if !at_word_boundary(text, cursor, needle) {
                    return None;
                }
                Some((needle, replacement))
            });
            match matched {
                Some((needle, replacement)) => {
                    let original = needle.to_string();
                    let start = out.len();
                    out.push_str(replacement);
                    edits.push(AppliedEdit {
                        out_start: start,
                        out_end: out.len(),
                        original,
                    });
                    cursor += needle.len();
                }
                None => {
                    let ch = text[cursor..].chars().next().expect("cursor is in range");
                    out.push(ch);
                    cursor += ch.len_utf8();
                }
            }
        }
        Corrected { text: out, edits }
    }
}

// ---------------------------------------------------------------------------
// 3. Indonesian normalization
// ---------------------------------------------------------------------------

/// The context guard a normalization rule can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Guard {
    /// Always applies (still word-boundary only).
    None,
    /// Only in a context that cannot be a foreign noun or a trailing hedge:
    /// not after a determiner, not clause-final, not inside an English run.
    SafeContext,
}

/// One conservative, reversible rewrite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NormalizationRule {
    pub id: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub guard: Guard,
}

/// The built-in rule table. Small on purpose: every entry is a colloquial form
/// with one standard written equivalent and no plausible English reading.
pub const NORMALIZATION_RULES: [NormalizationRule; 7] = [
    NormalizationRule {
        id: "gak",
        from: "gak",
        to: "tidak",
        guard: Guard::None,
    },
    NormalizationRule {
        id: "nggak",
        from: "nggak",
        to: "tidak",
        guard: Guard::None,
    },
    NormalizationRule {
        id: "kalo",
        from: "kalo",
        to: "kalau",
        guard: Guard::None,
    },
    NormalizationRule {
        id: "udah",
        from: "udah",
        to: "sudah",
        guard: Guard::None,
    },
    NormalizationRule {
        id: "gimana",
        from: "gimana",
        to: "bagaimana",
        guard: Guard::None,
    },
    NormalizationRule {
        id: "aja",
        from: "aja",
        to: "saja",
        guard: Guard::None,
    },
    // The plan's fixture: "kayak" is both the Indonesian colloquial "like" and
    // the English boat noun, so it only rewrites in an unambiguously
    // Indonesian, non-final position.
    NormalizationRule {
        id: "kayak",
        from: "kayak",
        to: "seperti",
        guard: Guard::SafeContext,
    },
];

/// English function words that mark a window as an English run.
const ENGLISH_FUNCTION_WORDS: [&str; 30] = [
    "the", "a", "an", "this", "that", "these", "those", "is", "are", "was", "were", "be", "been",
    "of", "to", "in", "on", "at", "and", "or", "but", "with", "for", "from", "i", "you", "we",
    "they", "it", "not",
];

/// Determiners and possessives that make a following token a noun phrase head.
const DETERMINERS: [&str; 24] = [
    "the", "a", "an", "this", "that", "these", "those", "my", "your", "his", "her", "its", "our",
    "their", "some", "any", "every", "no", "ini", "itu", "sang", "para", "sebuah", "seorang",
];

/// Opt-in normalization over [`NORMALIZATION_RULES`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Normalizer {
    pub enabled: bool,
}

impl Normalizer {
    pub fn new(enabled: bool) -> Self {
        Self { enabled }
    }

    pub fn rules(&self) -> &'static [NormalizationRule] {
        &NORMALIZATION_RULES
    }

    /// Applies the rules; a disabled normalizer returns the input unchanged and
    /// with no edits.
    pub fn apply(&self, text: &str) -> Corrected {
        if !self.enabled {
            return Corrected {
                text: text.to_string(),
                edits: Vec::new(),
            };
        }
        let tokens = token_spans(text);
        let mut out = String::with_capacity(text.len());
        let mut edits = Vec::new();
        let mut cursor = 0usize;
        for (position, (start, end)) in tokens.iter().copied().enumerate() {
            if start < cursor {
                continue;
            }
            let token = &text[start..end];
            let rule = NORMALIZATION_RULES
                .iter()
                .find(|rule| rule.from == token && passes_guard(rule, &tokens, position, text));
            // Copy everything between the last edit and this token verbatim.
            out.push_str(&text[cursor..start]);
            match rule {
                Some(rule) => {
                    let out_start = out.len();
                    out.push_str(rule.to);
                    edits.push(AppliedEdit {
                        out_start,
                        out_end: out.len(),
                        original: token.to_string(),
                    });
                }
                None => out.push_str(token),
            }
            cursor = end;
        }
        out.push_str(&text[cursor..]);
        Corrected { text: out, edits }
    }
}

/// The guards: word context for [`Guard::SafeContext`].
fn passes_guard(
    rule: &NormalizationRule,
    tokens: &[(usize, usize)],
    position: usize,
    text: &str,
) -> bool {
    match rule.guard {
        Guard::None => true,
        Guard::SafeContext => {
            // Clause-final: the token is the last one, or punctuation follows.
            if position + 1 == tokens.len() {
                return false;
            }
            let (_, end) = tokens[position];
            if text[end..]
                .chars()
                .next()
                .is_some_and(|ch| matches!(ch, '.' | ',' | '?' | '!' | ';' | ':'))
            {
                return false;
            }
            // …and only after a determiner (possessives included).
            if position > 0 {
                let (start, end) = tokens[position - 1];
                let previous = text[start..end].to_ascii_lowercase();
                if DETERMINERS.contains(&previous.as_str()) {
                    return false;
                }
            }
            // …skip inside an English-looking run (±3 tokens, ≥2 function words).
            let english = (position.saturating_sub(3)..=(position + 3).min(tokens.len() - 1))
                .map(|index| {
                    let (start, end) = tokens[index];
                    text[start..end].to_ascii_lowercase()
                })
                .filter(|token| ENGLISH_FUNCTION_WORDS.contains(&token.as_str()))
                .count();
            english < 2
        }
    }
}

// ---------------------------------------------------------------------------
// 4. Clean export preview
// ---------------------------------------------------------------------------

/// What the clean export may change. Every flag is off unless chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanOptions {
    /// Apply the stored dictionary.
    pub dictionary: bool,
    /// Apply the opt-in Indonesian normalization rules.
    pub normalize: bool,
    /// Collapse runs of spaces and trim each line (typographic only).
    pub collapse_whitespace: bool,
}

impl CleanOptions {
    /// Whether any transform is selected at all.
    pub fn any(&self) -> bool {
        self.dictionary || self.normalize || self.collapse_whitespace
    }
}

/// The rules a clean preview runs: the stored dictionary, and whether the
/// normalizer is opted in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextRules {
    pub dictionary: Dictionary,
    pub normalize: bool,
}

impl TextRules {
    pub fn is_empty(&self) -> bool {
        self.dictionary.is_empty() && !self.normalize
    }
}

/// One segment's before/after in the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRow {
    pub idx: u32,
    pub before: String,
    pub after: String,
}

impl PreviewRow {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

/// A computed clean-export preview. Pure data: nothing has been written, and
/// nothing is written until [`ExportPreview::apply`] is asked for the derived
/// transcript.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExportPreview {
    pub rows: Vec<PreviewRow>,
    /// Rows whose text would change (the ones the diff panel marks).
    pub changed: usize,
}

impl ExportPreview {
    pub fn is_empty(&self) -> bool {
        self.changed == 0
    }

    /// The one-line summary the sheet shows.
    pub fn summary(&self) -> String {
        if self.rows.is_empty() {
            return "no segments to preview".to_string();
        }
        if self.changed == 0 {
            return "nothing to clean in this transcript".to_string();
        }
        format!(
            "{} of {} segments would change — review, then Apply",
            self.changed,
            self.rows.len()
        )
    }

    /// The **derived** transcript the exporter would write. The input is not
    /// touched; the caller's stored text and `source_text` are unchanged.
    pub fn apply(&self, transcript: &Transcript) -> Transcript {
        let mut derived = transcript.clone();
        for (segment, row) in derived.segments.iter_mut().zip(self.rows.iter()) {
            if segment.idx == row.idx {
                segment.text = row.after.clone();
            }
        }
        derived
    }
}

/// Computes the clean preview. No I/O, no mutation, deterministic.
pub fn preview(
    transcript: &Transcript,
    rules: &TextRules,
    options: &CleanOptions,
) -> ExportPreview {
    let normalizer = Normalizer::new(options.normalize && rules.normalize);
    let mut rows = Vec::with_capacity(transcript.segments.len());
    let mut changed = 0usize;
    for segment in &transcript.segments {
        let mut text = segment.text.clone();
        if options.normalize && rules.normalize {
            text = normalizer.apply(&text).text;
        }
        if options.dictionary {
            text = rules.dictionary.apply(&text).text;
        }
        if options.collapse_whitespace {
            text = collapse_lines(&text);
        }
        if text != segment.text {
            changed += 1;
        }
        rows.push(PreviewRow {
            idx: segment.idx,
            before: segment.text.clone(),
            after: text,
        });
    }
    ExportPreview { rows, changed }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// `from = to`, `from -> to` or `from => to`.
fn split_pair(line: &str) -> Option<(&str, &str)> {
    for separator in ["->", "=>", "="] {
        if let Some((from, to)) = line.split_once(separator) {
            return Some((from.trim(), to.trim()));
        }
    }
    None
}

/// Trims and folds interior whitespace runs to one space.
fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Collapses interior space/tab runs (per line) and trims each line, keeping
/// hard line breaks exactly where the transcript put them.
fn collapse_lines(text: &str) -> String {
    text.split('\n')
        .map(collapse_spaces)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Word spans: maximal runs of alphanumerics (Unicode) and `_`. Combining marks
/// and apostrophes stay inside a token, so `don't` and a decomposed accent are
/// one word and no rule can fire inside them.
fn token_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start: Option<usize> = None;
    for (offset, ch) in text.char_indices() {
        let wordish = ch.is_alphanumeric() || ch == '_' || ch == '\'' || is_combining(ch);
        match (wordish, start) {
            (true, None) => start = Some(offset),
            (false, Some(begin)) => {
                spans.push((begin, offset));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        spans.push((begin, text.len()));
    }
    spans
}

/// Whether `needle` at `start` is a whole word of `text`.
fn at_word_boundary(text: &str, start: usize, needle: &str) -> bool {
    let end = start + needle.len();
    if end > text.len() {
        return false;
    }
    let before = text[..start].chars().next_back();
    let after = text[end..].chars().next();
    let wordish = |ch: char| ch.is_alphanumeric() || ch == '_' || is_combining(ch);
    !before.is_some_and(wordish) && !after.is_some_and(wordish)
}

/// Whether `haystack` contains `needle` as a whole word.
fn contains_word(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let mut start = 0usize;
    while let Some(offset) = haystack[start..].find(needle) {
        let at = start + offset;
        if at_word_boundary(haystack, at, needle) {
            return true;
        }
        start = at
            + haystack[at..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
    }
    false
}

/// A conservative combining-mark test (the dependency-free approximation used
/// by `diagnostics`: a mark attaches to the previous cluster).
fn is_combining(ch: char) -> bool {
    matches!(ch as u32,
        0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF | 0xFE20..=0xFE2F)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Segment, Source};

    fn transcript(texts: &[&str]) -> Transcript {
        Transcript::new(
            "glossary.wav".to_string(),
            Source::File {
                name: "glossary.wav".to_string(),
            },
            "tiny".to_string(),
            Some("id".to_string()),
            None,
            texts
                .iter()
                .enumerate()
                .map(|(idx, text)| Segment {
                    idx: idx as u32,
                    start_ms: idx as i64 * 1_000,
                    end_ms: idx as i64 * 1_000 + 900,
                    text: (*text).to_string(),
                })
                .collect(),
        )
    }

    // ---- the glossary --------------------------------------------------------

    #[test]
    fn the_glossary_parses_one_term_per_line_and_bounds_itself() {
        let glossary = Glossary::parse(
            "  Kecamatan Cilandak  \n\nSuku Dinas Pendidikan\nkecamatan cilandak\n\
             Jakarta Selatan",
        );
        assert_eq!(
            glossary.terms(),
            [
                "Kecamatan Cilandak",
                "Suku Dinas Pendidikan",
                "Jakarta Selatan"
            ],
            "trimmed, blanks skipped, the case-insensitive duplicate folded"
        );
        assert_eq!(glossary.len(), 3);
        assert_eq!(glossary.to_field().lines().count(), 3);

        // 64 terms fit; the 65th is counted as over the cap.
        let many: String = (0..70)
            .map(|index| format!("term {index}\n"))
            .collect::<String>();
        let glossary = Glossary::parse(&many);
        assert_eq!(glossary.len(), DEFAULT_GLOSSARY_CAP);
        assert_eq!(glossary.over_cap(), 70 - DEFAULT_GLOSSARY_CAP);

        // A term longer than the 48-character limit is rejected, never cut.
        let long = "x".repeat(MAX_TERM_CHARS + 1);
        let glossary = Glossary::parse(&format!("ok\n{long}\n"));
        assert_eq!(glossary.terms(), ["ok"]);
        assert_eq!(glossary.rejected().len(), 1);
        assert_eq!(glossary.rejected()[0].len(), MAX_TERM_CHARS + 1);
    }

    #[test]
    fn the_glossary_field_round_trips() {
        let original = Glossary::parse("Kecamatan Cilandak\nBPJS Kesehatan\n\nJakarta");
        let reparsed = Glossary::parse(&original.to_field());
        assert_eq!(original, reparsed);
    }

    // ---- prompt budgeting ----------------------------------------------------

    /// An exact counter for tests: one token per whitespace-separated word.
    fn words(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn the_measured_budget_is_half_the_models_text_context() {
        let budget = PromptBudget::measured("ggml-tiny", 448);
        assert_eq!(budget.cap_tokens(), 224);
        assert_eq!(
            budget.source(),
            &BudgetSource::Measured {
                model_id: "ggml-tiny".to_string(),
                n_text_ctx: 448
            }
        );
        assert!(budget.caption().contains("224 tokens measured"));
        assert!(budget.caption().contains("n_text_ctx 448"));

        // A different model measures differently — nothing is hardcoded.
        assert_eq!(
            PromptBudget::measured("ggml-large-v3", 448).cap_tokens(),
            224
        );
        assert_eq!(PromptBudget::measured("tiny.en", 0).cap_tokens(), 1);

        // The fallback says so, in words the settings surface shows verbatim.
        let fallback = PromptBudget::unmeasured();
        assert_eq!(fallback.cap_tokens(), FALLBACK_PROMPT_CAP_TOKENS);
        assert!(matches!(fallback.source(), BudgetSource::Fallback));
        assert!(fallback.caption().contains("documented fallback"));
    }

    #[test]
    fn the_builder_includes_terms_in_order_until_the_cap_and_reports_the_rest() {
        let glossary = Glossary::parse("alpha\nbravo\ncharlie\ndelta");
        // The word counter: "one two" = 2, each ", term" adds one.
        let budget = PromptBudget {
            cap_tokens: 5,
            source: BudgetSource::Fallback,
        };
        let plan = build_prompt(Some("one two"), &glossary, &budget, &words);
        assert_eq!(plan.text, "one two, alpha, bravo, charlie");
        assert_eq!(plan.included, ["alpha", "bravo", "charlie"]);
        assert_eq!(plan.dropped, ["delta"]);
        assert_eq!(plan.used_tokens, 5);
        assert!(plan.used_tokens <= plan.cap_tokens, "never over budget");
        assert!(plan.truncated());
        let feedback = plan.feedback().expect("feedback");
        assert!(
            feedback.contains("1 of 4 glossary terms dropped"),
            "{feedback}"
        );
        assert!(feedback.contains("capped at 5 tokens"), "{feedback}");
        assert_eq!(plan.prompt(), Some("one two, alpha, bravo, charlie"));
    }

    #[test]
    fn a_long_term_does_not_shut_out_the_short_ones_after_it() {
        let glossary = Glossary::parse(&format!("ab\n{}\ncd", "x".repeat(40)));
        let budget = PromptBudget {
            cap_tokens: 3,
            source: BudgetSource::Fallback,
        };
        let plan = build_prompt(None, &glossary, &budget, &EstimatedTokens);
        // "ab" fits; the 40-character term would need 15 tokens; "cd" still
        // gets a chance and fits.
        assert_eq!(plan.included, ["ab", "cd"]);
        assert_eq!(plan.dropped.len(), 1);
        assert_eq!(plan.dropped[0].len(), 40);
        // Deterministic: the same inputs always produce the same plan.
        assert_eq!(
            plan,
            build_prompt(None, &glossary, &budget, &EstimatedTokens)
        );
    }

    #[test]
    fn the_builder_never_rewrites_an_over_budget_prompt() {
        let glossary = Glossary::parse("alpha");
        let budget = PromptBudget {
            cap_tokens: 2,
            source: BudgetSource::Fallback,
        };
        let plan = build_prompt(Some("one two three four"), &glossary, &budget, &words);
        assert_eq!(plan.text, "one two three four", "kept verbatim");
        assert!(plan.base_included && plan.base_over_budget);
        assert!(plan.included.is_empty());
        assert_eq!(plan.dropped, ["alpha"]);
        let feedback = plan.feedback().expect("feedback");
        assert!(feedback.contains("already exceeds the cap"), "{feedback}");
    }

    #[test]
    fn an_empty_glossary_and_no_base_means_no_prompt() {
        let glossary = Glossary::default();
        let budget = PromptBudget::measured("tiny", 448);
        let plan = build_prompt(None, &glossary, &budget, &words);
        assert_eq!(plan.text, "");
        assert_eq!(plan.prompt(), None, "nothing to send");
        assert!(!plan.truncated());
        assert_eq!(plan.feedback(), None);
    }

    #[test]
    fn the_built_in_estimate_is_conservative() {
        let counter = EstimatedTokens;
        assert_eq!(counter.count(""), 0);
        // 3 bytes per token, rounded up: over-counts English (~4 bytes/token)
        // and Indonesian subwords, never under-counts wildly.
        assert_eq!(counter.count("abc"), 1);
        assert_eq!(counter.count("abcd"), 2);
        assert!(counter.count("Kecamatan Cilandak") >= 6);
    }

    // ---- the dictionary ------------------------------------------------------

    #[test]
    fn the_dictionary_parses_all_three_separators_and_refuses_junk() {
        let dict = Dictionary::parse("BPJS = Badan Penyelenggara Jaminan Sosial\nNIK -> Nomor Induk Kependudukan\nRT => rukun tetangga");
        assert_eq!(dict.len(), 3);
        assert!(dict.rejected().is_empty());
        let from: Vec<&str> = dict.entries().iter().map(|e| e.from.as_str()).collect();
        assert_eq!(from, ["BPJS", "NIK", "RT"]);

        let dict =
            Dictionary::parse("no separator here\n= missing from\nmissing to =\nsame = same\n");
        assert_eq!(dict.len(), 0);
        assert_eq!(dict.rejected().len(), 4);
    }

    #[test]
    fn the_dictionary_applies_whole_words_only_and_keeps_case_sensitive_matching() {
        let dict = Dictionary::parse("BPJS = Badan Penyelenggara Jaminan Sosial\nkayak = perahu");
        let out = dict.apply("Kartu BPJS dan BPJSnya, kayak kecil, Kayak besar.");
        assert_eq!(
            out.text,
            "Kartu Badan Penyelenggara Jaminan Sosial dan BPJSnya, perahu kecil, Kayak besar."
        );
        assert_eq!(out.edits.len(), 2, "two whole-word matches");
        assert_eq!(
            out.restore(),
            "Kartu BPJS dan BPJSnya, kayak kecil, Kayak besar."
        );
    }

    #[test]
    fn the_dictionary_is_idempotent_and_never_chains() {
        let dict = Dictionary::parse("a = b\nb = c");
        assert_eq!(dict.len(), 1, "the second entry would chain and is refused");
        assert_eq!(dict.rejected().len(), 1);

        // A legitimate pair: one pass, no rescan of its own output.
        let dict = Dictionary::parse("gak = tidak\ntidak = bukan");
        assert_eq!(dict.len(), 1, "tidak → bukan would chain on the next pass");
        let dict = Dictionary::parse("gak = tidak");
        let once = dict.apply("aku gak tahu").text;
        assert_eq!(once, "aku tidak tahu");
        assert_eq!(dict.apply(&once).text, once, "re-running is a no-op");
    }

    #[test]
    fn the_dictionary_round_trips_through_its_edit_log() {
        let dict =
            Dictionary::parse("BPJS = Badan Penyelenggara Jaminan Sosial\nRT = rukun tetangga");
        for original in [
            "Kartu BPJS di RT 05.",
            "tanpa aturan apa pun",
            "BPJS",
            "RT RT RT",
            "emoji 🎙️ dan BPJS",
        ] {
            let corrected = dict.apply(original);
            assert_eq!(
                corrected.restore(),
                original,
                "exact inverse for {original:?}"
            );
            // …and the text-level inverse agrees where `to` is unambiguous.
            let inverse = dict.apply_inverse(&corrected.text);
            assert_eq!(inverse.text, original, "inverse rules for {original:?}");
        }
    }

    #[test]
    fn the_dictionary_field_round_trips() {
        let dict =
            Dictionary::parse("BPJS = Badan Penyelenggara Jaminan Sosial\nRT = rukun tetangga");
        assert_eq!(Dictionary::parse(&dict.to_field()), dict);
    }

    // ---- Indonesian normalization -------------------------------------------

    fn normalizer() -> Normalizer {
        Normalizer::new(true)
    }

    #[test]
    fn the_normalizer_rewrites_colloquial_indonesian_whole_words() {
        let out = normalizer().apply("Aku gak tahu, kalo dia udah pulang aja.");
        assert_eq!(out.text, "Aku tidak tahu, kalau dia sudah pulang saja.");
        assert_eq!(out.edits.len(), 4);
        assert_eq!(
            out.restore(),
            "Aku gak tahu, kalo dia udah pulang aja.",
            "the edit log restores the exact input"
        );

        // No substring rewrites: "gak" inside "nggak" is its own rule, and a
        // longer token is never touched.
        let out = normalizer().apply("nggak apa-apa, gakgak bukan kata");
        assert_eq!(out.text, "tidak apa-apa, gakgak bukan kata");
        // A disabled normalizer is exactly the identity.
        let off = Normalizer::new(false).apply("gak kalo udah");
        assert_eq!(off.text, "gak kalo udah");
        assert!(off.edits.is_empty());
    }

    #[test]
    fn the_kayak_fixture_preserves_the_english_noun_and_the_trailing_hedge() {
        // English: a determiner before and a clause-final position after.
        for english in [
            "The kayak drifted downstream.",
            "We rented a kayak in Ubud.",
            "I packed my kayak, then left.",
        ] {
            let out = normalizer().apply(english);
            assert_eq!(out.text, english, "the English boat noun is preserved");
            assert!(out.edits.is_empty());
        }
        // The Indonesian trailing hedge: clause-final, so preserved.
        let out = normalizer().apply("Dia bilang gitu, kayak.");
        assert_eq!(out.text, "Dia bilang gitu, kayak.");
        // Clause-final without punctuation (end of text) is the same case.
        let out = normalizer().apply("terus gitu kayak");
        assert_eq!(out.text, "terus gitu kayak");

        // …but the colloquial "like" in a safe, non-final position rewrites.
        let out = normalizer().apply("Dia kayak gitu terus");
        assert_eq!(out.text, "Dia seperti gitu terus");
        // …unless it sits inside an English-looking run.
        let out = normalizer().apply("She said the thing is kayak gitu");
        assert_eq!(
            out.text, "She said the thing is kayak gitu",
            "two English function words mark the window as an English run"
        );
    }

    #[test]
    fn code_switching_is_respected_token_by_token() {
        // An English clause keeps its words; the Indonesian clause normalizes.
        let out = normalizer().apply("The meeting is at four; aku gak bisa datang.");
        assert_eq!(
            out.text, "The meeting is at four; aku tidak bisa datang.",
            "the English run is untouched, the Indonesian token is not"
        );
        // A code-switched noun phrase after a determiner stays.
        let out = normalizer().apply("Saya lihat the kayak itu di pantai");
        assert_eq!(out.text, "Saya lihat the kayak itu di pantai");

        // A hyphen is a word boundary, so a hyphenated form's first element is
        // still its own word; an underscore is not, so `gak_bisa` is one token
        // and no rule can fire inside it.
        let out = normalizer().apply("gak-bisa dan gak_bisa");
        assert_eq!(out.text, "tidak-bisa dan gak_bisa");
    }

    #[test]
    fn normalization_is_idempotent_and_reversible_on_every_fixture() {
        let cases = [
            "Aku gak tahu, kalo dia udah pulang aja.",
            "Dia kayak gitu terus",
            "The kayak drifted downstream.",
            "gimana kabar?",
            "emoji 🎙️ gak masalah",
        ];
        for case in cases {
            let once = normalizer().apply(case);
            let twice = normalizer().apply(&once.text);
            assert_eq!(twice.text, once.text, "re-running is a no-op: {case:?}");
            assert_eq!(once.restore(), case, "exact inverse: {case:?}");
        }
    }

    // ---- the clean preview ---------------------------------------------------

    #[test]
    fn the_clean_preview_is_a_diff_and_nothing_else() {
        let transcript = transcript(&["Kartu BPJS aku gak tahu", "sudah bersih"]);
        let rules = TextRules {
            dictionary: Dictionary::parse("BPJS = Badan Penyelenggara Jaminan Sosial"),
            normalize: true,
        };
        let options = CleanOptions {
            dictionary: true,
            normalize: true,
            collapse_whitespace: false,
        };
        let preview = preview(&transcript, &rules, &options);
        assert_eq!(preview.rows.len(), 2);
        assert_eq!(preview.changed, 1);
        assert!(preview.rows[0].changed());
        assert_eq!(
            preview.rows[0].after,
            "Kartu Badan Penyelenggara Jaminan Sosial aku tidak tahu"
        );
        assert_eq!(preview.rows[1].before, preview.rows[1].after);
        assert!(preview.summary().contains("1 of 2 segments would change"));

        // The preview wrote nothing: the transcript is byte-identical.
        assert_eq!(transcript.segments[0].text, "Kartu BPJS aku gak tahu");

        // Apply returns a DERIVED copy; the original stays untouched.
        let derived = preview.apply(&transcript);
        assert_eq!(
            derived.segments[0].text,
            "Kartu Badan Penyelenggara Jaminan Sosial aku tidak tahu"
        );
        assert_eq!(transcript.segments[0].text, "Kartu BPJS aku gak tahu");
    }

    #[test]
    fn verbatim_mode_changes_nothing_and_whitespace_cleanup_is_its_own_choice() {
        let transcript = transcript(&["dua  spasi   di sini"]);
        let rules = TextRules {
            dictionary: Dictionary::parse("dua = 2"),
            normalize: true,
        };
        // Verbatim: every option off.
        let verbatim = preview(&transcript, &rules, &CleanOptions::default());
        assert_eq!(verbatim.changed, 0);
        assert_eq!(
            verbatim.apply(&transcript).segments[0].text,
            "dua  spasi   di sini"
        );

        // Whitespace-only cleanup opts in separately from the dictionary.
        let whitespace = CleanOptions {
            collapse_whitespace: true,
            ..CleanOptions::default()
        };
        let preview = preview(&transcript, &rules, &whitespace);
        assert_eq!(preview.rows[0].after, "dua spasi di sini");
    }

    #[test]
    fn the_clean_pipeline_is_idempotent() {
        let transcript = transcript(&["Kartu BPJS,  aku gak tahu "]);
        let rules = TextRules {
            dictionary: Dictionary::parse("BPJS = Badan Penyelenggara Jaminan Sosial"),
            normalize: true,
        };
        let options = CleanOptions {
            dictionary: true,
            normalize: true,
            collapse_whitespace: true,
        };
        let once = preview(&transcript, &rules, &options);
        let cleaned = once.apply(&transcript);
        let twice = preview(&cleaned, &rules, &options);
        assert_eq!(
            twice.changed, 0,
            "cleaning an already clean transcript is a no-op"
        );
    }
}
