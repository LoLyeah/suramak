# M15-A — Workbench core: M15.1 + M15.2 + M15.4

Status: complete, gates green. Scope from `docs/WORKORDER-M15A.md`; parent plan
`docs/PLAN-M15-M18.md` §3 (M15 slices 1, 2, 4). No app version bump (v0.4.0 is
the owner's call when all of M15 lands). No commit was made.

## Result in one line

The store now keeps the inference text beside every editable segment (additive
schema v2, in-place upgrade, backup/restore round-trip proven), the Opus import
path is pinned by EN+ID fixtures across Ogg/WebM/MKV with named errors on
truncated input and a real mid-decode cancel seam, and the editor carries
advisory CPL/CPS badges plus an ASS exporter that libass renders correctly.

## Gates

| gate | result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (1 m 38 s) |
| `cargo test --workspace` | **619 passed / 0 failed / 8 ignored** (baseline 586/0/8; +33) |
| new fixtures | 13 fixture files + README + `SHA256SUMS`, 144 KB directory total (pins verified with `sha256sum -c`) |
| disk | `target/` 4.4 GB after removing the incremental cache; 17 GB free on `/` |

The full workspace suite was run as the final gate (`cargo test --workspace`)
after the last edit; the touched crates were exercised individually during
development.

## Slice 1 — M15.1 persistence and provenance (S)

Deliverable: `docs/inventory/m15-persistence.md` (schema, write paths,
transaction coverage, derived text, search, alignment, counting rules, decode
contract). Written against live code; no speculative paths.

### The one real source-text-loss path — fixed

`segments.text` was the only copy of the inference output. The editor, autosave
and accepted polish all call `Store::update_segment_text`, which overwrote it in
place; the one-step polish undo only restores the pre-polish buffer, so the
original inference wording was gone after the first edit. That is exactly what
the plan's data rule forbids ("preserve original inference text; corrections are
derived outputs").

Fix, additive only:

- schema **v1 → v2**: `ALTER TABLE segments ADD COLUMN source_text TEXT`;
- backfill `source_text = text` for existing rows (the only copy a v1 database
  has), inside one transaction; `meta.schema_version` → `2`;
- `insert_transcript` writes both columns; `upsert_segments`' conflict branch
  uses `COALESCE(segments.source_text, excluded.source_text)` so no polish or
  edit can overwrite the first text; new rows get their own first text;
- `Store::segment_source_text(id, idx)` reads it; no UI shows it yet;
- a database from a **newer** schema is refused (`SuramakError::Schema`) rather
  than guessed at; a fresh file is created at v2 directly.

No table was rebuilt, no column dropped, no row deleted. `update_segment_text`'s
two statements were also put in one savepoint (previously autocommit): a failure
after the text write could leave new text with an old `updated_at` — now both
roll back (unit test with a `BEFORE UPDATE` trigger).

### Backup / restore (new, library-only)

`Store::backup_to` uses `VACUUM INTO` through the live connection: consistent
snapshot (WAL included), one compact file, no sidecars, refuses to overwrite.
`Store::restore_from` validates `PRAGMA integrity_check` and the schema version
read-only, copies to `<db>.restore-tmp`, fsyncs, removes stale `-wal`/`-shm`,
then renames over the target (Unix atomic replace; Windows falls back to
remove-then-rename because `std::fs::rename` refuses an existing destination, and
a crash between the two leaves the validated temp file, not a corrupt database);
the caller must hold no open `Store` (documented). No UI or CLI calls either
function yet — the gate exercises them in tests.

### Verification (the slice's own gate)

`crates/suramak-core/tests/persistence.rs`, 7 tests:

- a populated **v1** library is built *in-test* from the exact DDL app 0.3.0/
  0.3.1 shipped (raw SQL through the store's own SQLite binding; no committed
  binary), then `Store::open` upgrades it in place: both transcripts, every
  segment, EN+ID text, emoji and a decomposed accent survive byte-for-byte;
  `source_text` is backfilled for every row; a second open is a no-op; the
  physical column exists;
- editor and polish rewrites keep the first stored text; a new polish row
  records its own; a removed tail row loses its provenance with it;
- backup → mutate (edit + delete) → restore returns every row, exact text and
  source_text; restoring again is fine; an existing backup is never overwritten;
- a corrupt backup is refused with `SuramakError::Backup` and the live library
  is untouched; no `restore-tmp` is left behind;
- a newer-schema database is refused;
- search behaviour is pinned (below).

### Search — findings (nothing was changed)

- Only `Store::search` exists. It is a **literal LIKE substring** scan over
  `transcripts.title` + `segments.text` (`%…%`, `ESCAPE '\'`, wildcards
  literal), `DISTINCT`, newest first. **No FTS5**, no index, no snippets, no
  offsets/timestamps; results are transcript headers.
- Case folding is ASCII-only (SQLite default): `SELAMAT` matches `Selamat`;
  `éclair` does not match `Éclair`.
- It searches the **current** text, not `source_text`; an edit that removes a
  word removes it from results while the original is still stored.
- **No product caller**: the UI has no search field and `suramak-cli library`
  only has `list`; only tests call `search`. M15.7 starts from a tested store
  API with zero UI wiring — it should decide deliberately between keeping the
  LIKE scan and adding FTS5 (and then keep English stemming away from
  Indonesian retrieval).

### Honest boundaries

- `source_text` is stored and readable but not surfaced in the editor (an
  "original vs edited" view is a later slice); there is no revision history,
  only the first text.
- Backup/restore has no UI, scheduling, retention or encryption.
- The UI's multi-segment autosave is still one savepoint per row, not one for
  the batch (`runtime::save_edits` lives outside this workorder's touchable
  set); a mid-batch failure keeps earlier rows and retries the rest.
- Derived polish text has no model/provenance pointer yet; translation does not
  exist in the tree (M15.8 will need its own additive migration).
- Alignment metadata is segment start/end only; there is no word-timestamp
  shape to preserve or migrate.

## Slice 2 — M15.2 import parity and shared decode (S)

### Fixtures (data only)

`crates/suramak-core/fixtures/audio/` — 6 complete + 6 truncated + 1 malformed,
140 KB total. Opus is encoded **once per language** (48 kHz mono, 24 kbit/s,
voice profile, 4 s trims of the repo's EN/ID TTS samples) and remuxed with
`-c:a copy` into WebM and Matroska, so all three containers carry identical
packets — parity means bit-identical PCM, not "sounds similar". Exact ffmpeg
8.0.1 commands, the source text and sha256 pins are in `README.md` +
`SHA256SUMS` (verified). ffmpeg is fixture tooling only; it is not a product
dependency and never runs in the app.

### Parity result

`crates/suramak-core/tests/decode_parity.rs`, 8 tests, all through the shipped
`symphonia` + `symphonia-adapter-libopus` path:

- EN: Ogg = WebM = MKV, PCM SHA-256
  `0dd17233ecf4e20aefabb417025c402f24ec3cec916984206f20f43b30bccb0e` (4.013 s);
- ID: Ogg = WebM = MKV, PCM SHA-256
  `273628e4d4adf75e8800ce700da00fb6618817ba02d732eef426ae16ccb5d905` (4.013 s);
- the two languages differ (a copy-paste fixture error fails loudly).

Hash input: mono mixdown, 48 kHz normalization, little-endian f32. All fixtures
are already 48 kHz mono (Opus decodes at 48 kHz by definition), so the helper's
mixdown/resample are guards rather than conversions; "varying input rates"
cannot be exercised through Opus and is covered by the WAV/MP3/AAC fixtures and
resampler tests.

### Truncated / malformed input

- Ogg-Opus (the WhatsApp/Discord voice-note shape) hides its length in a cut
  file. `decode_file` now checks the tail: the last **CRC-valid** page must be
  complete and carry the EOS flag; otherwise `SuramakError::Truncated` *before*
  any decode.
- Matroska/WebM declares the Segment size in the header; the new EBML check
  compares `segment_start + declared_size` with the file length. A 60 % prefix
  fails as `Truncated` before symphonia sees it.
- Defence in depth: the packet loop and the probe run behind `catch_unwind`.
  `symphonia-format-mkv` 0.5.5 **panics** in its EBML reader on truncated input
  (`EBML header must be read before calling this function`); that is now a
  named `SuramakError::DecoderPanic` (message says the file may be corrupt),
  never a process abort. The default panic hook still prints one line to stderr;
  the app survives.
- Malformed bytes keep the actionable `UnsupportedFormat` error.
- Remaining honest gap: containers with no declared length (headerless MP3, a
  live Ogg without EOS, Matroska with unknown Segment size) cannot be proven
  truncated structurally; after decode, a declared `n_frames` (100 ms
  tolerance) catches most of them. This is documented, not hidden.

### Cancellation

`decode_file_cancellable(path, &|| token.is_cancelled())` (and the bytes form)
consults the closure before the probe and once per demuxed packet; the first
`true` returns `SuramakError::Cancelled` and drops all samples. The test cancels
on the sixth check of a 4 s file (after four packets) and asserts ≥ 6
consultations — proof it stopped mid-loop; a pre-cancelled token stops after one
check. This is the same cooperative shape as `suramak_asr::CancelToken`; the
desktop pipeline's `prepare_samples` still uses its stage-boundary check because
`suramak-asr` is outside this workorder's touchable set. Wiring the existing
token into decode is a one-line change for M13-C or a later M15 slice.

### Shared-decode contract for Android M13-C (one paragraph)

`suramak_core::decode::decode_file` — or `decode_file_cancellable(path, &||
token.is_cancelled())` — is the single import decoder for every platform: it
probes with symphonia (wav/mp3/flac/ogg-vorbis/m4a/mp4/mov/m4v/mkv/webm),
registers the bundled libopus adapter, reads the first real audio track,
returns interleaved `f32` with the container's rate/channels, rejects
structurally truncated Ogg and Matroska with `SuramakError::Truncated`, converts
foreign-decoder panics into `SuramakError::DecoderPanic`, and honours a
cooperative cancel between packets with `SuramakError::Cancelled`. M13-C must
copy SAF/share bytes to a local `files/media/<uuid>.<ext>` file and call this
function on it — never `MediaCodec`, a JNI decode, or a Kotlin resampler — so
the same containers, the same 48 kHz Opus path, the same named errors and the
same cancellation hold on device; the only device-specific work is the byte copy
and keeping the extension.

### Honest boundaries

- Parity is proven for identical packets across containers, not for
  independently re-encoded files (those legitimately produce different PCM).
- No Android code was touched (per the workorder); the contract is documentation
  for M13-C.
- Opus is always 48 kHz, so rate variance is not exercised here.

## Slice 3 — M15.4 CPL/CPS badges + basic ASS (S)

### Diagnostics

`suramak_core::diagnostics`:

- grapheme-aware counts (combining marks, variation selectors, emoji modifiers,
  tag chars, keycap marks, ZWJ sequences, regional-indicator pairs) — a bounded,
  dependency-free UAX #29 approximation documented in the module;
- CPL = longest hard line after per-line trim; interior spaces count; no
  implicit wrapping;
- CPS = total graphemes ÷ duration; `end <= start` → `zero_duration` flag and
  no CPS judgement;
- configurable `SubtitleLimits`, defaults 37/42 CPL and 17/20 CPS, strict
  `>` comparison (37 OK, 38 warning, 42 warning, 43 error).

The editor paints a quiet badge in each row's spare width (`CPL 44`,
`CPS 20.5`, or both), amber for warning and the text-safe record red for error,
with a hover explanation. It reads the live editable buffer, so it updates while
typing. Nothing truncates or blocks. Thresholds come from
`AppState::subtitle_limits`, so a settings surface can replace them without
touching the counting code; **no settings UI ships in this slice**.

Tests: 6 core unit tests (grapheme sequences, boundaries, zero/negative
duration, configurable + swapped thresholds, badge composition) and 2 UI tests
(severity mapping incl. a decomposed `café` counted as 80 graphemes not 100
chars, long Indonesian word counted letter by letter; badge ink tokens).

### ASS serializer

`export::to_ass` / `to_ass_with(&AssOptions)` — one `Dialogue` per segment from
the same text the SRT/VTT paths read; `[Script Info]` 1920×1080, `WrapStyle: 2`
(no re-wrap), one `Default` style (Arial 48, white fill, black outline, bottom
centre); `H:MM:SS.cc` timestamps rounded to the nearest centisecond.

- **Zero duration** (`end <= start`): never emitted. Default
  `ZeroDuration::OneFrame` widens to 40 ms (25 fps) so the line stays readable
  and the timing defect stays visible; `Skip` drops the cue (documented, opt-in).
  Negative starts clamp to 0.
- **Escaping, only what the spec requires**: `{`/`}` → `\{`/`\}` (otherwise they
  open an override block and the text vanishes); real newlines → `\N`; a
  literal backslash is preserved except immediately before `N`/`n`/`h`, where a
  zero-width space is inserted — ASS has no backslash escape, so this is the
  only visually lossless way to keep `\N`; commas need no escaping because
  `Text` is the last Dialogue field.
- Wired into the existing UI export surface: `ExportFormat::Ass` (sixth chip,
  `.ass`, default destination follows the format); hint text updated. No new UI
  flow, no CLI change (the CLI's own `export --format` list stays
  md/txt/json/srt/vtt; out of the touchable set).

### Independent playback verification (what was actually done)

Both available methods, not just the fallback:

1. **libass 0.17.4 (ffmpeg 8.0.1)** rendered our generated ASS to PNG at several
   timestamps and converted it to SRT:
   `ffmpeg -y -f lavfi -i color=c=black:s=1280x720:r=1 -t 10 -vf "ass=ours.ass" our%02d.png`
   and `ffmpeg -y -i ours.ass ours.srt` (a throwaway dump test generated
   `ours.ass` through the shipped serializer; the dump test was removed after
   the check). Observed: the two-line cue renders as two lines; `{not a tag}`
   renders with literal braces; a literal `\N` renders as `\N` (ZWSP invisible);
   `path\to\file` keeps its backslashes; the SRT conversion preserved cue times,
   including the zero-duration cue widened to `00:00:06,000 → 00:00:06,040`.
   This was a one-off verification (ffmpeg is not a test dependency); the
   serializer's own tests pin the text.
2. **In-cargo**: a hand-rolled parser over our own output (10-field Dialogue
   split, `\N`/brace/ZWSP unescaping, centisecond parse) checks round-trip text,
   escaping, zero-duration policy, negative starts, hour-long timestamps, and
   compares the words with the SRT path (`ass_and_srt_carry_the_same_words`).
   6 tests in `crates/suramak-core/tests/export.rs` plus the golden.

### Honest boundaries

- One style, no font/colour/size configuration, no bilingual stacked tracks, no
  karaoke tags (needs M15.6 word timing), no positioning/margins UI.
- `WrapStyle: 2` means the text's own `\N` breaks are the only breaks; a
  renderer will not re-wrap a long single line (the CPL badge is the advisory
  for that).
- Centisecond rounding can move a cue boundary by ≤5 ms; cue order and positive
  duration are preserved.
- The literal-backslash-before-`N` case inserts a zero-width space — visually
  identical, not byte-identical, and documented as the format's limit.
- ASS was verified with libass (the reference implementation), not with VLC or
  Aegisub on macOS; a QuickTime/AVFoundation ASS reader does not exist, so the
  owner's Mac player check remains a smoke item, not a code gate.

## Dependency and surface impact

- **No dependencies added, removed or version-bumped.** `Cargo.toml` is
  untouched; the tests use crates already in the product graph (`rusqlite`,
  `sha2`, `uuid`, `tempfile`).
- Files touched: `suramak-core/src/{store,decode,export,error,lib}.rs`, new
  `diagnostics.rs`; `suramak-core/tests/{persistence,decode_parity,export}.rs`;
  `suramak-ui/src/app/{editor,export,mod}.rs`; new
  `crates/suramak-core/fixtures/audio/`; `docs/inventory/m15-persistence.md`;
  this report.
- The four Mac-facing surfaces (record/transcribe/editor/library) still compile
  and test; the only UI change is the badge in the existing segment row and the
  sixth export chip.

## Follow-ups this slice deliberately leaves

1. Surface `source_text` in the editor (original vs edited) and let search see
   it (M15.7).
2. Wire `decode_file_cancellable` into `suramak-asr::prepare_samples` with the
   job's `CancelToken` (one line; outside the touchable set here), and have
   M13-C call the same function.
3. Decide LIKE-scan vs FTS5 for M15.7, and give the library a search field —
   today the store API has no UI caller at all.
4. A settings surface for the CPL/CPS thresholds (`AppState::subtitle_limits`
   is ready).
5. Derive-text provenance (polish model id, translation source revision) when
   M15.5/M15.8 land; they must not reuse `source_text` for derived output.
