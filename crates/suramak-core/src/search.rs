//! Search shaping and snippets (M15.7) — the pure half of `Store`'s FTS5
//! search, with no SQL in it.
//!
//! ## Query shaping
//!
//! User input is **never** handed to FTS5 as an expression. [`match_query`]
//! splits it on whitespace and turns every surviving word into a quoted FTS5
//! string, joined by the implicit AND. That makes `-`, `"`, `(`, `:`, `OR`,
//! `NEAR` and friends ordinary text instead of operators, so a query can never
//! fail with an FTS5 syntax error and a user cannot accidentally ask for an
//! OR. Two documented conveniences remain:
//!
//! - a word keeps its interior punctuation (`e-mail` is an FTS5 phrase of two
//!   adjacent tokens, which is what the tokenizer would have done anyway);
//! - a trailing `*` is an explicit prefix search (`pegawai*` finds
//!   `pegawairungu`). Nothing else is a wildcard.
//!
//! A word with no alphanumeric character left (`-`, `…`) is dropped; a query
//! with nothing left is not searched at all (`None`), never an error.
//!
//! ## Snippets
//!
//! FTS5's own `snippet()` counts **tokens**, but the workorder asks for a
//! configurable context window in **characters**, so [`snippet`] builds the
//! window itself:
//!
//! - the text is whitespace-collapsed first (a merged segment's `\n` becomes a
//!   single space), so a snippet is always one line;
//! - the window is `context` chars before and after the earliest query term;
//!   the term is located case- and diacritic-insensitively, on word
//!   boundaries, mirroring the `unicode61 remove_diacritics 1` tokenizer
//!   (accented Latin-1 letters fold to their base letter, so `cafe` finds
//!   `café`, as the index does);
//! - the window edges snap outward to word boundaries (never further than
//!   [`WORD_SNAP_CHARS`]) so a snippet does not start mid-word, but never
//!   inside the match itself;
//! - a cut edge is marked with a single `…` (U+2026 HORIZONTAL ELLIPSIS, no
//!   surrounding spaces). An ellipsis therefore means "the text continues",
//!   never "the segment is empty";
//! - when no term can be located (an unusual script the fold table does not
//!   cover, or a tokenizer split we cannot reproduce), the snippet is the head
//!   of the text and [`Snippet::highlight`] is `None` — the row still shows its
//!   match, it just cannot mark it.

use uuid::Uuid;

/// Context chars on each side of the match when the caller does not choose.
pub const DEFAULT_CONTEXT_CHARS: usize = 80;
/// The smallest accepted context window (a clipped word is worse than no
/// snippet, so this is deliberately not 0).
pub const MIN_CONTEXT_CHARS: usize = 10;
/// The largest accepted context window — beyond this the "snippet" is the
/// segment, and the list row cannot show it anyway.
pub const MAX_CONTEXT_CHARS: usize = 400;
/// How far a snippet edge may travel to snap to a word boundary.
pub const WORD_SNAP_CHARS: usize = 24;
/// The longest single query word kept (a pasted paragraph is not a query).
pub const MAX_TERM_CHARS: usize = 64;
/// Default number of hits a caller gets when it does not choose.
pub const DEFAULT_LIMIT: usize = 50;
/// The largest accepted hit limit.
pub const MAX_LIMIT: usize = 200;

/// What one search row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitKind {
    /// A segment of a transcript, with its start time and snippet.
    Segment,
    /// A transcript title.
    Title,
    /// A glossary term.
    Glossary,
    /// A dictionary entry (`from` → `to`).
    Dictionary,
}

impl HitKind {
    /// The string stored in `search_docs.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            HitKind::Segment => "segment",
            HitKind::Title => "title",
            HitKind::Glossary => "glossary",
            HitKind::Dictionary => "dictionary",
        }
    }

    /// Parses a stored kind; an unknown value is `None` (skipped, not guessed).
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "segment" => Some(HitKind::Segment),
            "title" => Some(HitKind::Title),
            "glossary" => Some(HitKind::Glossary),
            "dictionary" => Some(HitKind::Dictionary),
            _ => None,
        }
    }
}

/// How a caller wants its results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchOptions {
    /// Chars of context on each side of the match (clamped to
    /// [`MIN_CONTEXT_CHARS`]..=[`MAX_CONTEXT_CHARS`]).
    pub context_chars: usize,
    /// At most this many hits (clamped to 1..=[`MAX_LIMIT`]).
    pub limit: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            context_chars: DEFAULT_CONTEXT_CHARS,
            limit: DEFAULT_LIMIT,
        }
    }
}

impl SearchOptions {
    /// The context window this call will actually use.
    pub fn context(&self) -> usize {
        self.context_chars
            .clamp(MIN_CONTEXT_CHARS, MAX_CONTEXT_CHARS)
    }

    /// The limit this call will actually use.
    pub fn limit(&self) -> usize {
        self.limit.clamp(1, MAX_LIMIT)
    }
}

/// One result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub kind: HitKind,
    /// The transcript a `Segment`/`Title` hit belongs to; `None` for the
    /// glossary/dictionary corpus, which is not tied to a recording.
    pub transcript_id: Option<Uuid>,
    /// The transcript title (for the result row's second line).
    pub title: Option<String>,
    /// The segment index, for `Segment` hits.
    pub segment_idx: Option<u32>,
    /// The segment's span, for `Segment` hits (`None` if the row vanished
    /// between the index and the store — an inconsistency the store's
    /// self-check repairs, not something to invent).
    pub start_ms: Option<i64>,
    pub end_ms: Option<i64>,
    /// The display text: a windowed snippet for segment/title hits, the term
    /// or `from → to` pair for the corpus.
    pub snippet: String,
    /// Byte span of the matched word inside `snippet`, when it could be
    /// located (the UI tints it; no other consumer needs it).
    pub highlight: Option<(usize, usize)>,
    /// Whether the snippet came from `source_text` (the preserved inference
    /// text) instead of the current `text`.
    pub from_source_text: bool,
}

/// Everything one search returned.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchResults {
    pub hits: Vec<SearchHit>,
    /// All documents that matched, before the limit — `total > hits.len()`
    /// means the list was cut.
    pub total: usize,
}

impl SearchResults {
    /// Whether more rows matched than were returned.
    pub fn truncated(&self) -> bool {
        self.total > self.hits.len()
    }
}

/// A shaped FTS5 query: the expression to bind to `MATCH`, plus the words it
/// is made of (the snippet builder re-finds them in the text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchQuery {
    pub expression: String,
    pub terms: Vec<String>,
}

/// Shapes user input into an FTS5 `MATCH` expression, or `None` when there is
/// nothing searchable in it.
pub fn match_query(input: &str) -> Option<MatchQuery> {
    let mut expression = String::new();
    let mut terms = Vec::new();
    for raw in input.split_whitespace() {
        let prefix = raw.ends_with('*');
        let mut cleaned: String = raw
            .chars()
            .filter(|c| *c != '"' && *c != '*')
            .take(MAX_TERM_CHARS)
            .collect();
        cleaned = trim_to_alnum(&cleaned);
        if cleaned.is_empty() {
            continue;
        }
        if !expression.is_empty() {
            expression.push(' ');
        }
        expression.push('"');
        expression.push_str(&cleaned);
        expression.push('"');
        if prefix && cleaned.chars().last().is_some_and(|c| c.is_alphanumeric()) {
            expression.push('*');
        }
        terms.push(cleaned);
    }
    if terms.is_empty() {
        return None;
    }
    Some(MatchQuery { expression, terms })
}

/// Trims characters that are not letters/digits from both ends, keeping the
/// interior untouched (`100%` → `100`, `--halo--` → `halo`).
fn trim_to_alnum(text: &str) -> String {
    let start = text
        .char_indices()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(i, _)| i);
    let Some(start) = start else {
        return String::new();
    };
    let end = text
        .char_indices()
        .rev()
        .find(|(_, c)| c.is_alphanumeric())
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(start);
    text[start..end.max(start)].to_string()
}

/// A built snippet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Snippet {
    pub text: String,
    /// Byte span of the matched word inside `text`.
    pub highlight: Option<(usize, usize)>,
}

/// Builds a one-line snippet around the earliest occurrence of any of
/// `terms` in `text`. See the module docs for the exact rules.
pub fn snippet(text: &str, terms: &[String], context_chars: usize) -> Snippet {
    let context = context_chars.clamp(MIN_CONTEXT_CHARS, MAX_CONTEXT_CHARS);
    let clean = collapse_whitespace(text);
    if clean.is_empty() {
        return Snippet::default();
    }
    let chars: Vec<char> = clean.chars().collect();
    let found = find_term(&chars, terms);

    let (mut start, mut end) = match found {
        Some((match_start, match_end)) => (
            match_start.saturating_sub(context),
            (match_end + context).min(chars.len()),
        ),
        None => (0, context.min(chars.len())),
    };

    // Snap outward to word boundaries, never over the match itself.
    if start > 0 {
        if chars[start - 1].is_alphanumeric() && chars[start].is_alphanumeric() {
            let limit = (start + WORD_SNAP_CHARS).min(chars.len());
            let mut i = start;
            while i < limit && !chars[i].is_whitespace() {
                i += 1;
            }
            if i < limit {
                start = i;
            }
        }
        while start < chars.len() && chars[start].is_whitespace() {
            start += 1;
        }
        if let Some((match_start, _)) = found {
            start = start.min(match_start);
        }
    }
    if end < chars.len() {
        if chars[end - 1].is_alphanumeric() && chars[end].is_alphanumeric() {
            let floor = end.saturating_sub(WORD_SNAP_CHARS);
            let mut i = end;
            while i > floor && !chars[i - 1].is_whitespace() {
                i -= 1;
            }
            if i > floor {
                end = i;
            }
        }
        while end > 0 && chars[end - 1].is_whitespace() {
            end -= 1;
        }
        if let Some((_, match_end)) = found {
            end = end.max(match_end);
        }
    }

    let mut out = String::with_capacity((end - start) * 4 + 8);
    if start > 0 {
        out.push('…');
    }
    let window_byte_start = out.len();
    out.extend(&chars[start..end]);
    let highlight = found.map(|(match_start, match_end)| {
        let prefix = |index: usize| {
            chars[start..index]
                .iter()
                .map(|c| c.len_utf8())
                .sum::<usize>()
        };
        (
            window_byte_start + prefix(match_start),
            prefix(match_end) - prefix(match_start),
        )
    });
    if end < chars.len() {
        out.push('…');
    }
    Snippet {
        text: out,
        highlight,
    }
}

/// Collapses every whitespace run to one space and trims the ends, so a
/// snippet is one display line regardless of the segment's stored newlines.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            pending_space = true;
        } else {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        }
    }
    out
}

/// The earliest word-boundary occurrence of any term: `(start, end)` in chars.
fn find_term(chars: &[char], terms: &[String]) -> Option<(usize, usize)> {
    let folded_terms: Vec<Vec<char>> = terms
        .iter()
        .map(|term| term.chars().map(fold_char).collect())
        .collect();
    let mut best: Option<(usize, usize)> = None;
    for at in 0..chars.len() {
        if at > 0 && chars[at - 1].is_alphanumeric() {
            continue; // a match must start at a word boundary
        }
        for term in &folded_terms {
            if term.is_empty() || at + term.len() > chars.len() {
                continue;
            }
            let end = at + term.len();
            if end < chars.len() && chars[end].is_alphanumeric() {
                continue; // the match must end at a word boundary
            }
            if term
                .iter()
                .zip(&chars[at..end])
                .all(|(wanted, actual)| *wanted == fold_char(*actual))
                && best.is_none_or(|(start, _)| at < start)
            {
                best = Some((at, end));
            }
        }
    }
    best
}

/// Case + Latin-1 diacritic folding, mirroring `unicode61 remove_diacritics 1`
/// for the scripts EN/ID transcripts actually use. A character outside the
/// table falls back to its simple lowercase form.
fn fold_char(c: char) -> char {
    let lower = c.to_lowercase().next().unwrap_or(c);
    match lower {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_word_is_quoted_so_fts5_syntax_is_text() {
        let q = match_query("Selamat OR jadwal").expect("a query");
        assert_eq!(q.expression, "\"Selamat\" \"OR\" \"jadwal\"");
        assert_eq!(q.terms, ["Selamat", "OR", "jadwal"]);

        // Quotes and operators inside a word are stripped, never interpreted.
        let q = match_query("say\"a -jakarta (rapat)").expect("a query");
        assert_eq!(q.expression, "\"saya\" \"jakarta\" \"rapat\"");
    }

    #[test]
    fn punctuation_only_input_is_not_a_query() {
        assert_eq!(match_query(""), None);
        assert_eq!(match_query("   "), None);
        assert_eq!(match_query("- -- …"), None);
        assert_eq!(match_query("\"\""), None);
    }

    #[test]
    fn a_trailing_star_is_a_prefix_search_and_nothing_else_is() {
        let q = match_query("pegawai*").expect("a query");
        assert_eq!(q.expression, "\"pegawai\"*");
        let q = match_query("*pegawai*").expect("a query");
        assert_eq!(q.expression, "\"pegawai\"*", "stars are edges, not syntax");
    }

    #[test]
    fn a_query_word_keeps_its_interior_punctuation() {
        let q = match_query("e-mail").expect("a query");
        assert_eq!(q.expression, "\"e-mail\"");
        assert_eq!(q.terms, ["e-mail"]);
    }

    #[test]
    fn a_short_text_is_its_own_snippet_without_ellipses() {
        let s = snippet("halo dunia", &["dunia".into()], 80);
        assert_eq!(s.text, "halo dunia");
        assert_eq!(s.highlight, Some((5, 5)));
        assert_eq!(&s.text[5..10], "dunia");
    }

    #[test]
    fn a_cut_edge_is_marked_with_one_ellipsis() {
        let text = "satu dua tiga empat lima enam tujuh delapan sembilan sepuluh";
        let s = snippet(text, &["enam".into()], 10);
        assert!(s.text.starts_with('…'), "{}", s.text);
        assert!(s.text.ends_with('…'), "{}", s.text);
        assert!(s.text.contains("enam"), "{}", s.text);
        let (start, len) = s.highlight.expect("the match is marked");
        assert_eq!(&s.text[start..start + len], "enam");
        // Exactly one leading and one trailing ellipsis.
        assert!(!s.text.starts_with("……"));
        assert!(!s.text.ends_with("……"));
    }

    #[test]
    fn a_match_at_the_start_has_no_leading_ellipsis() {
        let text = "halo dunia yang sangat panjang sekali dan terus berlanjut";
        let s = snippet(text, &["halo".into()], 12);
        assert!(!s.text.starts_with('…'), "{}", s.text);
        assert!(s.text.ends_with('…'), "{}", s.text);
    }

    #[test]
    fn matching_is_case_and_diacritic_insensitive_like_the_index() {
        let s = snippet("Éclair di café dekat sini", &["cafe".into()], 80);
        assert!(s.text.contains("café"), "{}", s.text);
        let (start, len) = s.highlight.expect("marked");
        assert_eq!(&s.text[start..start + len], "café");

        let s = snippet("Selamat PAGI semua", &["pagi".into()], 80);
        let (start, len) = s.highlight.expect("marked");
        assert_eq!(&s.text[start..start + len], "PAGI");
    }

    #[test]
    fn a_term_only_matches_on_a_word_boundary() {
        // "saya" must not be found inside "sayang": the result would point at
        // the wrong word while FTS5 never matched it.
        let s = snippet("sayang sekali", &["saya".into()], 80);
        assert_eq!(s.highlight, None);
        assert_eq!(s.text, "sayang sekali");
    }

    #[test]
    fn whitespace_and_newlines_collapse_into_one_line() {
        let s = snippet("baris satu\n\nbaris   dua", &["dua".into()], 80);
        assert_eq!(s.text, "baris satu baris dua");
        assert!(!s.text.contains('\n'));
        let (start, len) = s.highlight.expect("marked");
        assert_eq!(&s.text[start..start + len], "dua");
    }

    #[test]
    fn an_unlocatable_term_falls_back_to_the_head_without_a_mark() {
        let s = snippet("αβγδε ζηθικ λμνξο", &["πρσ".into()], 20);
        assert_eq!(s.highlight, None);
        assert_eq!(s.text, "αβγδε ζηθικ λμνξο");
    }

    #[test]
    fn the_earliest_of_several_terms_wins() {
        let s = snippet(
            "alpha beta gamma delta",
            &["gamma".into(), "beta".into()],
            80,
        );
        let (start, len) = s.highlight.expect("marked");
        assert_eq!(&s.text[start..start + len], "beta");
    }

    #[test]
    fn an_empty_text_has_an_empty_snippet() {
        assert_eq!(snippet("", &["x".into()], 40), Snippet::default());
        assert_eq!(snippet("   \n ", &["x".into()], 40), Snippet::default());
    }

    #[test]
    fn the_context_window_never_cuts_the_match() {
        // A long Indonesian word inside a sentence: a 10-char window can only
        // start at the word, never in the middle of it.
        let text = "kami membahas pertanggungjawaban anggaran besok pagi";
        let s = snippet(text, &["pertanggungjawaban".into()], 10);
        let (start, len) = s.highlight.expect("marked");
        assert!(start + len <= s.text.len());
        assert_eq!(&s.text[start..start + len], "pertanggungjawaban");
    }

    #[test]
    fn hit_kind_round_trips_through_its_stored_string() {
        for kind in [
            HitKind::Segment,
            HitKind::Title,
            HitKind::Glossary,
            HitKind::Dictionary,
        ] {
            assert_eq!(HitKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(HitKind::parse("nonsense"), None);
    }
}
