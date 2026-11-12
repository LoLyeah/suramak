//! M22C — the FTS audit gate, as a test.
//!
//! The workorder's question: *does a phrase search across 200 transcripts stay
//! under 50 ms on a 2-core box?* This builds the realistic corpus (200
//! transcripts × 40 segments = 8,000 segments, ~400k words), indexes it through
//! the ordinary write path, then times 20 representative queries (single
//! terms, ANDs, quoted phrases, speaker filters, and a mixture) against the
//! real `Store::search_segments`.
//!
//! p95 must be **under 50 ms**. If this assert fails the bar is not lowered:
//! the numbers are printed and the test fails, which is the gate's answer.

use std::path::PathBuf;
use std::time::Instant;

use suramak_core::diarization::{DiarizationRun, Speaker, SpeakerTurn};
use suramak_core::store::Store;
use suramak_core::transcript::{Segment, Source, Transcript};

/// 200 transcripts.
const TRANSCRIPTS: usize = 200;
/// 40 segments each: 8,000 segments.
const SEGMENTS: usize = 40;
/// ~50 words per segment: ~400,000 words.
const WORDS_PER_SEGMENT: usize = 50;
/// The audit's ceiling, in milliseconds (workorder: p95 < 50 ms).
const P95_CEILING_MS: f64 = 50.0;

/// A deterministic mixed EN/ID vocabulary, close enough to a real library that
/// the tokenizer and ranker see repeated, ordinary words rather than UUIDs.
const WORDS: &[&str] = &[
    "rapat",
    "anggaran",
    "jadwal",
    "peluncuran",
    "tim",
    "desain",
    "jakarta",
    "bandung",
    "besok",
    "pagi",
    "sore",
    "kantor",
    "proyek",
    "klien",
    "laporan",
    "hasil",
    "diskusi",
    "keputusan",
    "catatan",
    "suara",
    "mikrofon",
    "rekaman",
    "kuartal",
    "depan",
    "produk",
    "pengguna",
    "penelitian",
    "wawancara",
    "pasar",
    "harga",
    "kesepakatan",
    "mitra",
    "lokasi",
    "perjalanan",
    "acara",
    "undangan",
    "peserta",
    "pertanyaan",
    "jawaban",
    "ringkasan",
    "tindak",
    "lanjut",
    "risiko",
    "peluang",
    "anggaran",
    "rapat",
    "jadwal",
    "tim",
    "hasil",
    "diskusi",
    "kantor",
    "jakarta",
    "proyek",
    "klien",
    "desain",
    "produk",
    "catatan",
    "rekaman",
    "pagi",
    "besok",
];

fn segment_text(transcript: usize, segment: usize) -> String {
    let mut words = Vec::with_capacity(WORDS_PER_SEGMENT);
    // A unique opening pair keeps every row distinguishable without polluting
    // the query vocabulary.
    words.push(format!("baris{transcript}x{segment}"));
    words.push("catatan".to_string());
    for index in 0..WORDS_PER_SEGMENT - 2 {
        words.push(WORDS[(transcript * 7 + segment * 13 + index * 3) % WORDS.len()].to_string());
    }
    // Every fifth segment carries the phrase the phrase query asks for, and
    // every seventh the AND pair, so the audit measures real matching.
    if segment % 5 == 0 {
        words[4] = "anggaran".to_string();
        words[5] = "kuartal".to_string();
    }
    if segment % 7 == 0 {
        words[6] = "jadwal".to_string();
        words[7] = "peluncuran".to_string();
    }
    // Plant each audit query's words adjacent somewhere, so every timed query
    // is a real match rather than a fast empty answer.
    let planted: &[(usize, [&str; 3])] = &[
        (3, ["tim", "desain", "produk"]),
        (4, ["laporan", "hasil", "diskusi"]),
        (6, ["jakarta", "bandung", "besok"]),
        (8, ["proyek", "klien", "baru"]),
        (9, ["rekaman", "suara", "pagi"]),
        (10, ["diskusi", "catatan", "rapat"]),
        (11, ["kantor", "besok", "pagi"]),
        (12, ["hasil", "diskusi", "tim"]),
        (13, ["kuartal", "depan", "ini"]),
    ];
    for (modulus, phrase) in planted {
        if segment % modulus == 0 {
            words[2] = phrase[0].to_string();
            words[3] = phrase[1].to_string();
            words[4] = phrase[2].to_string();
        }
    }
    words.join(" ")
}

fn corpus() -> Vec<Transcript> {
    (0..TRANSCRIPTS)
        .map(|t| {
            let segments = (0..SEGMENTS)
                .map(|s| Segment {
                    idx: s as u32,
                    start_ms: s as i64 * 4_000,
                    end_ms: s as i64 * 4_000 + 3_900,
                    text: segment_text(t, s),
                })
                .collect();
            Transcript::new(
                format!("Synthetic interview {t:03}"),
                Source::File {
                    name: format!("synthetic-{t:03}.wav"),
                },
                "base".to_string(),
                Some("id".to_string()),
                None,
                segments,
            )
        })
        .collect()
}

/// The 20 audit queries: `(label, query)`. Each must match at least one row,
/// so a silently empty search cannot pass by being fast.
fn queries() -> [(&'static str, &'static str); 20] {
    [
        ("single common", "rapat"),
        ("single mid", "anggaran"),
        ("single rarer", "mikrofon"),
        ("phrase", "\"anggaran kuartal\""),
        ("phrase 2", "\"tim desain\""),
        ("AND 2", "jadwal peluncuran"),
        ("AND 3", "laporan hasil diskusi"),
        ("AND mixed", "jakarta bandung"),
        ("AND rare", "proyek klien"),
        ("speaker only", "speaker:Budi"),
        ("speaker + term", "speaker:Sari rapat"),
        ("speaker + phrase", "speaker:Budi \"anggaran kuartal\""),
        ("phrase + term", "\"rekaman suara\" pagi"),
        ("two terms", "kuartal depan"),
        ("two terms 2", "diskusi catatan"),
        ("two terms 3", "kantor besok"),
        ("prefix", "pelunc*"),
        ("prefix 2", "desain*"),
        ("phrase 3", "\"hasil diskusi\""),
        ("speaker + common", "speaker:Budi anggaran"),
    ]
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() as f64 * fraction).ceil() as usize)
        .saturating_sub(1)
        .min(sorted.len() - 1);
    sorted[index]
}

fn db_path() -> PathBuf {
    std::env::temp_dir().join(format!("suramak-m22c-audit-{}.db", std::process::id()))
}

#[test]
fn two_hundred_transcripts_search_under_fifty_milliseconds_at_p95() {
    let path = db_path();
    let _ = std::fs::remove_file(&path);
    let store = Store::open(&path).expect("store");
    let corpus = corpus();

    let build = Instant::now();
    for transcript in &corpus {
        store.insert_transcript(transcript).expect("insert");
    }
    // A realistic speaker annotation on the first 50 transcripts (the rest are
    // the plain, un-diarized majority a library also has).
    for (index, transcript) in corpus.iter().take(50).enumerate() {
        let run = DiarizationRun::new(
            transcript.id,
            0,
            "segmentation",
            "a",
            "embedding",
            "b",
            2,
            0.5,
            (SEGMENTS as i64) * 4_000,
        );
        let speakers = [Speaker::new(0, "Budi", 0), Speaker::new(1, "Sari", 1)];
        let half = (SEGMENTS / 2) as i64 * 4_000;
        let turns = [
            SpeakerTurn::new(0, 0, half, 0),
            SpeakerTurn::new(1, half, SEGMENTS as i64 * 4_000, 1),
        ];
        store
            .replace_diarization_run(transcript.id, &run, &speakers, &turns)
            .expect("diarization");
        let _ = index;
    }
    let build_ms = build.elapsed().as_secs_f64() * 1_000.0;

    assert_eq!(store.segment_index_rows().unwrap(), 8_000);
    let words: usize = corpus
        .iter()
        .flat_map(|transcript| &transcript.segments)
        .map(|segment| segment.text.split_whitespace().count())
        .sum();
    assert!(words >= 390_000, "the corpus is ~400k words: {words}");

    // Warm one pass (statement cache, FTS pages) and verify every query has
    // real results before timing it.
    let queries = queries();
    let mut warm_hits = Vec::with_capacity(queries.len());
    for (label, query) in queries {
        let hits = store.search_segments(query, 50).expect("warm");
        assert!(!hits.is_empty(), "{label}: {query:?} must match the corpus");
        assert!(hits.len() <= 50);
        warm_hits.push(hits.len());
    }

    let mut samples = Vec::with_capacity(queries.len());
    let mut per_query = Vec::with_capacity(queries.len());
    for (label, query) in queries {
        let start = Instant::now();
        let hits = store.search_segments(query, 50).expect("search");
        let elapsed_ms = start.elapsed().as_secs_f64() * 1_000.0;
        std::hint::black_box(hits.len());
        samples.push(elapsed_ms);
        per_query.push((label, elapsed_ms, hits.len()));
    }
    let mut sorted = samples.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = percentile(&sorted, 0.50);
    let p95 = percentile(&sorted, 0.95);
    let max = *sorted.last().unwrap();
    let min = *sorted.first().unwrap();
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;

    let db_bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
    println!(
        "M22C AUDIT: {TRANSCRIPTS} transcripts × {SEGMENTS} segments = 8000 segments, \
         {words} words | db {:.1} MB | build {:.0} ms | 20 queries: \
         min {min:.2} ms, p50 {p50:.2} ms, p95 {p95:.2} ms, max {max:.2} ms, mean {mean:.2} ms",
        db_bytes as f64 / 1_048_576.0,
        build_ms
    );
    for (label, ms, hits) in &per_query {
        println!("  {label:<20} {ms:>6.2} ms  {hits:>2} hits");
    }

    assert!(
        p95 < P95_CEILING_MS,
        "M22C audit failed: p95 {p95:.2} ms is not under the {P95_CEILING_MS:.0} ms bar \
         (p50 {p50:.2} ms, max {max:.2} ms) — do not lower the bar"
    );

    drop(store);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_file(path.with_extension("db-wal"));
    let _ = std::fs::remove_file(path.with_extension("db-shm"));
}
