# M23 — speaker tagging (per recording, never identity)

Owner's question: *"while I said manual speaker naming before, is it possible
to tag 'Speaker 1', 'Speaker 2', 'Speaker 3', and etc for different speaker?"*
Carbon high answered (full text in the appendix).

## The answer, plainly

**Yes — and it costs nothing ethically.** Per-recording tagging groups turns of
one file into `Speaker 1…N`; the embeddings live in RAM for that run and are
discarded. It is not voiceprint memory, so it sits inside the rule the owner
set ("manual naming only" = no stored gallery, no cross-file suggestions).

**Where the product actually stands (verified in code):**

- The engine exists: sherpa-onnx pyannote-seg + CAM++, auto cluster count,
  `default_label()` already yields `Speaker N`; the store has
  `rename_speaker`, `set_speaker_color`, `merge_speakers`, `split_speaker`
  on time-anchored turns, and segment ranges are derived on read.
- Desktop can run it (`SpeakerAction::Run`) with an availability probe.
- **Android cannot**: `suramak-asr` is compiled without the `parakeet` /
  `vad-chunking` features there, so sherpa is absent and no diarization path
  exists. The phone renders speaker headers and notification prefixes only
  when a run already exists (i.e. made on desktop).
- **Exports carry no speakers at all** (`export.rs` has no speaker path).

## Why manual first, automatic second

Carbon's slice-1 proposal (compile sherpa for `arm64-v8a`, then pass a phone
gate at RTF ≤ 0.45, RSS ≤ +180 MB, battery ≤ 10%) is right in principle.
(The dependency fear written here first — "sherpa-rs ships prebuilt natives
per target and Android is not one of its advertised targets" — turned out to
be **wrong**: `sherpa-rs-sys 0.6.8` ships an Android prebuilt set. See
`docs/reports/M23b-android-diarization-gate.md`, verdict **BORDERLINE**, for
the two real blockers: 4 KB-page-aligned ONNX Runtime in the pinned 1.12.9
build, and the in-memory waveform's memory profile for long recordings.) The
owner's literal ask (tag Speaker 1/2/3) still does not need the engine, which
is why manual tagging shipped first.

So:

1. **M23A — manual tagging that works everywhere today** (no engine): a
   Speakers sheet with Add/Rename/Colour/Merge/Clear, tap-a-turn assignment,
   split-on-pause, undo, and speaker-aware exports (SRT/VTT/MD + `Include
   speakers`), plus the "edited since export" honesty note.
2. **M23B — the Android diarization gate** (a report, not product code):
   prove or disprove that sherpa-rs can be built/linked for `arm64-v8a`; if
   it can, run the measured phone gate on the owner's device and only then
   write the product slice. If it cannot, the phone stays manual and desktop
   keeps automatic — stated honestly instead of promised.

## Interaction model (phone), from Carbon + parent trim

- Editor: coloured badges + 4 px left rail per turn; a bottom-bar **Speakers**
  button reading `Tag speakers` (untagged) or `3 speakers`.
- Tap a speaker header → **Speaker sheet**: rename, colour dots, merge into…,
  remove label (reverts those turns to unlabelled).
- Tap a segment → **turn strip**: speaker chips + `+ New`, scope toggle
  `This turn | From here down`; two taps to assign or split; Undo.
- Speakers sheet: list, add, re-run (desktop only), clear all speakers.
- Colours are bound at creation and never re-indexed: a merge keeps the
  target's colour, a split claims the next unused slot.
- Exports: `Include speakers` toggle; SRT `Speaker 1: …`, VTT
  `<v Speaker 1>…`, MD `**Speaker 1:** …`; after a later rename the export
  sheet says "Speakers edited since last export — export again to update".

## Non-goals (explicit)

Voiceprints / persisted embeddings, per-app speaker profiles, cross-file
suggestions, auto-naming from contacts, cloud diarization. (Owner declined
the first; the rest would be the same cliff in a different coat.)

---

# Appendix — Carbon high (gemini-4-carbon, thinking 12288), verbatim

### A. Plain Answer

**Yes.** Tagging `"Speaker 1"`, `"Speaker 2"`, `"Speaker 3"` inside one recording fits your rule `[INFERRED]`. You declined voiceprint memory—storing embeddings to recognise people across files `[KNOWN]`. Per-recording diarization computes temporary embeddings in RAM to group turns within *one file*, labels them `"Speaker 1…N"`, and discards the embeddings `[INFERRED]`.

Suramak already has this engine (`pyannote segmentation-3.0` + `CAM++` via `sherpa-onnx`) and SQLite store (`speakers` table, `"Speaker N"` defaults, time-anchored turns) on desktop `[KNOWN]`. Android already renders speaker headers and share-to-notification prefixes, lacking only the compiled engine and touch controls `[KNOWN]`. Product contract: **per-recording only, automatic on request, quick to fix by hand, never cross-file identity** `[INFERRED]`.

---

### B. Phone Interaction Model (`egui` Android)

1. **Transcript Screen**: Keeps existing coloured badges + `[● Speaker 1]` headers `[KNOWN]` with a 4 px left rail `[INFERRED]` (slimmer than desktop's 8 px `[KNOWN]`). Bottom bar gets one `Speakers` button (`Tag speakers` when untagged; `3 speakers` when tagged) `[INFERRED]`. Unlabelled segments show a muted `[+ Speaker]` pill `[INFERRED]`.
2. **Speaker Sheet (tap any `[● Speaker 1]` header)**: Thumb-zone bottom sheet `[INFERRED]`:
   * **Rename**: Text field commits `rename_speaker` `[KNOWN]` across this note; colour dots call `set_speaker_color` `[KNOWN]`.
   * **Merge**: Tap `Merge into…` → tap `Speaker 2` (`merge_speakers` `[KNOWN]`). Shows a 6 s `Merged [Undo]` toast `[INFERRED]`.
   * **Drop cluster**: `Remove label` reverts that cluster's turns to unlabelled `[INFERRED]`.
3. **Turn Bar (tap a segment or `[+ Speaker]` pill)**: Bottom strip with chips `[Speaker 1] [Speaker 2] [+ New]` and scope toggle `This turn | From here down` (maps to `split_speaker` / updating time-anchored turns `[KNOWN]`, UI `[INFERRED]`). Handles **splitting** a wrongly-merged speaker and **assigning unlabelled segments** in two taps, with Undo `[INFERRED]`.
4. **Speakers Sheet (tap bottom-bar `Speakers`)**: Lists all speakers, `Re-run`, and `Clear all speakers` (**diarized by mistake**) `[INFERRED]`. Because segment ranges are derived on read from time-anchored turns `[KNOWN]`, deleting the note's `speakers`/`speaker_turns` rows restores plain text without touching Whisper segments (with session Undo) `[INFERRED]`.

---

### C. Automatic vs Manual on the Phone

Desktop runs at `0.150–0.167 s` CPU/audio-s and `135–152 MB` RSS `[KNOWN]`. Mid-range ARM64 phones under sustained load typically run ~2–3× slower: `0.30–0.50 s/audio-s` (`18–30 min` for 60-min audio) `[INFERRED]`.

On-device auto-tagging is viable *because* jobs already run in a durable, resumable FGS with progress `[KNOWN]`—**if** Whisper weights unload before `sherpa-onnx` allocates `135–152 MB` `[INFERRED]`.

* **Decision Rule & Gate**: Cross-compile `sherpa-onnx` (`arm64-v8a`) and run a 60-min multi-speaker recording on your **real Android phone**, unplugged, screen off (never an emulator—host x86 hides ARM64 thermals, battery, and `LMKD` behaviour `[INFERRED]`):
  * **Pass**: RTF ≤ `0.45×` (≤ 27 min), native RSS delta ≤ `180 MB` (zero OS kills), battery drop ≤ `10%`, thermal slowdown `< 25%` `[INFERRED]` → ship auto + manual.
  * **Borderline (`0.45–0.80×` RTF)**: Ship opt-in per note; warn on files >30 min on battery `[INFERRED]`.
  * **Fail (`>0.80×` RTF or OOM)**: Fallback to **manual tagging only** on phone—split turns on Whisper pauses `>1.2 s` and expose `[+ Speaker]` pills (desktop still diarizes) `[INFERRED]`.

---

### D. Cluster-Count UX & Failure Remedies

Auto-clustering (`[KNOWN]`) miscounts `N` when voices sound similar or rooms echo `[INFERRED]`.

* **Ask lightly before, never block background jobs**: Tapping `Tag speakers` opens a compact sheet: `How many people spoke? [Auto] [2] [3] [4+]` (`Auto` default) + `Tag` `[INFERRED]`. Asking *before* gives exact `N` on common 2- or 3-person recordings and avoids re-running; asking *after* wastes a 20-min phone run `[INFERRED]`.
* **Fix wrong `N` afterwards without re-running** (via `[KNOWN]` store ops; zero stored embeddings):
  * **Over-splitting** (one voice → Speakers 1 & 3): Speaker 3 sheet → 1-tap `Merge into Speaker 1` `[INFERRED]`.
  * **Under-splitting** (two voices → Speaker 1): Tap turn → Turn Bar → `+ New` (`From here down` or `This turn`) `[INFERRED]`. *(If interleaved throughout, `Re-run` with explicit `[2]`/`[3]` re-runs diarization only, keeping Whisper text `[INFERRED]`.)*
  * **Short turns ("Yeah", "Mm")**: Noisy sub-second embeddings `[INFERRED]` → tap turn → 1-tap neighbour speaker chip `[INFERRED]`.
  * **Crosstalk**: Keep dominant voice; skip dual-speaker overlap UI—1-tap chip if misassigned `[INFERRED]`.
  * **Laughter / noise**: Phantom `Speaker 4` → Speaker 4 sheet → 1-tap `Remove label` `[INFERRED]`.

---

### E. Labels, Colours, & Exports

* **Labels**: Show `"Speaker 1"`, `"Speaker 2"` `[KNOWN]` ordered by first appearance `[INFERRED]`; switch immediately to custom names on `rename_speaker` `[KNOWN]` within that file only `[INFERRED]`.
* **Colour stability**: Bind each speaker to its colour slot `[KNOWN]` at creation and never re-index `[INFERRED]`:
  * *Merge (`3 → 1`)*: Turns take `Speaker 1`'s name and colour; `Speaker 2`'s colour never shifts `[INFERRED]`.
  * *Split (`1 → 1 + new`)*: `Speaker 1` keeps its colour; new speaker claims next unused slot `[INFERRED]`.
* **Exports (SRT / VTT / MD)**: Export current labels (`Speaker 1` or `Budi`) behind an `Include speakers` toggle—SRT: `Speaker 1: …`, VTT: `<v Speaker 1>…`, MD: `**Speaker 1:** …` `[INFERRED]`.
* **Rename after export**: Never silently rewrite exported files `[KNOWN]`. Exports are snapshots; if renamed later, show *"Speakers edited since last export — export again to update"* in the Export sheet `[INFERRED]`.

---

### F. Slices & Non-Goals

* **First Slice (≤1 week — phone smile test)**:
  1. Compile `sherpa-onnx` diarization for Android `arm64-v8a`, unload Whisper before diarization, pass the phone gate `[INFERRED]`. *(Day-2 fallback if NDK/gate fails: ship pause-split manual tagging.)*
  2. Wire `Tag speakers` (`Auto`) into the existing FGS queue `[KNOWN]` + add **Speaker Sheet** (`Rename`, `Merge`, `Clear all`) `[INFERRED]`.
  * *Acceptance test*: On your Android phone, run `Tag speakers` on a 2-person recording → FGS completes with progress → editor shows coloured `Speaker 1` / `Speaker 2` → rename `Speaker 1`, trigger share-to-notification `[KNOWN]` → notification shows renamed prefixes; SQLite/disk holds zero voice embeddings `[INFERRED]`.
* **Second Slice**: `[Auto | 2 | 3 | 4+]` selector, **Turn Bar** (`Split` / `[+ Speaker]` manual assignment), colour picker (`set_speaker_color` `[KNOWN]`), Undo toast `[INFERRED]`.
* **Explicitly DO NOT build**:
  * Voiceprints or persisted voice embeddings `[KNOWN]`.
  * Per-app speaker profiles or cross-file suggestions `[KNOWN]`.
  * Cloud diarization `[INFERRED]`.
  * Auto-naming from