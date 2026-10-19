# M15–M18 — Post-launch milestone plans

Status: owner-review draft (2026-10-19). No workorder is cut from this document until approved slice by slice.
Provenance: drafted by **openai/gpt-6.1-sol** (OpenRouter) at the owner's request from the verified competitive research (2026-10-19: drafted by an agy teamwork run, salvaged and verified by Hermes — the research itself is kept out of the repo by owner request) and grounded/edited by Hermes against the live repo. M13 Android and M14 Windows remain the owner-approved launches; their research-derived refinements live as addenda in their own plan docs.

## Decision

Preserve the owner-approved M13 Android and M14 Windows launch plans. Add only import, presentation, and verification refinements that fit their existing slices; do not turn either launch into a capture-engine expansion.

Organize subsequent work into four gated milestones:

| Milestone | Name | Primary outcome |
|---|---|---|
| **M15** | **Transcript Workbench** | Faster correction, search, and bilingual subtitle preparation |
| **M16** | **Capture & Speakers** | Permission-aware dictation, additional desktop capture, and offline speaker labels |
| **M17** | **Local Polish** | Optional offline refinement and evidence-grounded meeting notes |
| **M18** | **Optional Media Tools** | Independently gated sharing, speaker-directory, and acoustic-event experiments |

These are proposed scope decisions, not delivery-date commitments. M18 is a holding milestone: deferred items require an owner go/no-go before implementation.

## 1. Integration principles

### Acceptance gates

Accept a feature only when it clears three gates:

1. **Owner value:** it improves a concrete workflow — importing a voice note, correcting EN+ID speech, finding a passage, preparing subtitles, or understanding speaker turns.
2. **Constraint fit:** it preserves native Rust and `egui`/`wgpu`, the Deck language, GPL-3.0 compatibility, zero telemetry, GitHub-only distribution, and local-first operation. Network access remains limited to user-initiated pinned model downloads and the user's explicitly enabled Polish endpoint.
3. **Dependency order:** packaging, schema integrity, cancellation, and platform permissions precede dependent UI promises. A competitor's feature is evidence of a useful pattern, not evidence that Suramak's backend already supports it.

EN and Indonesian receive separate verification, including code-switching, long Indonesian words, names, colloquial usage, and noisy audio. Indonesian must never silently route to an English-only fast engine.

### Scope and data rules

- Preserve original inference text. Normalization, correction, translation, and summaries are explicit derived outputs, not destructive replacements.
- Treat diarization as session-local clustering, not verified personal identity.
- Treat word timestamps as estimated alignment, not guaranteed millisecond accuracy.
- Keep endpoint use visibly opt-in. No translation request occurs merely because a bilingual export panel opens.
- Every added model requires license review, SHA-256 pinning, resumable-download compatibility, and corrupt-download handling through Model Manager.
- New native libraries require reproducible packaging and license review; the existing `sherpa-onnx` opt-in use does not prove release readiness.
- Keep motion first-class: recording transitions, progress, and playback highlighting follow Deck conventions. Respect reduced-motion preferences and bound animation work; do not introduce continuous idle redraws.
- Measure startup, idle memory, and active-engine memory separately. The research's **<60 MB idle** and **<120 ms cold start** figures are targets, not established measurements.

Local processing reduces off-device exposure; it does **not** guarantee legal compliance or eliminate recording-consent obligations. Unverified market-size and voice-note statistics are not acceptance criteria.

### Workorder sizing

Sizes apply to individual dispatchable workorders, not entire competitive features:

- **S:** one bounded UI, serializer, rule, or adapter change with focused tests.
- **M:** a cross-layer change involving persistence, audio, permissions, or platform integration.
- **L:** an uncertain engine/runtime integration. It must begin with a bounded feasibility workorder and split before production implementation.

Each workorder names its deliverable, verification, and prerequisite. Failed feasibility gates cause explicit deferral, not silent milestone expansion.

## 2. Placement table

The research inventory contains **25 surface-level entries** despite its "22 features" shorthand; all are accounted for below.

**Status:** ACCEPT = retain the core feature; ACCEPT-ADAPTED = narrow or correct it; DEFER = reserve a slot without committing implementation; REJECT = do not ship the proposed form.

| Candidate | Decision | Placement / size | Reason |
|---|---|---|---|
| Global hotkey + floating HUD | ACCEPT-ADAPTED | M16 capture slices / M per platform | Permission-aware adapters with explicit manual-copy fallback; universal insertion is not assumed. |
| WhatsApp/Opus ingestion | ACCEPT-ADAPTED | M15 import parity / S | Desktop Opus already ships (M10C native libopus); the remaining work is fixture parity + Android reuse of the same decode path. |
| System-audio loopback | ACCEPT-ADAPTED | M16 platform capture / M each | macOS already ships through `suramak-taps`; only Linux and Windows are new implementations. |
| Word-level karaoke + seek | ACCEPT-ADAPTED | M15 alignment slices / M | Estimated word alignment with segment fallback; token probabilities alone are insufficient. |
| CPL/CPS badges | ACCEPT | M15 subtitle diagnostics / S | Configurable advisory guardrails, including Indonesian layout fixtures. |
| Split/merge shortcuts | ACCEPT | M15 editing transactions / M | Include undo, search consistency, and timing invalidation — not just key bindings. |
| Mini-waveform | ACCEPT | M16 waveform / M | Bounded audio cache and tactile boundary editing improve correction. |
| Speaker colors/lanes | ACCEPT-ADAPTED | M16 speaker integration / S | Restrained badges/tints after diarization; no full timeline editor. |
| FTS5 search + seek snippets | ACCEPT-ADAPTED | M15 search enhancement / M | Inspect the existing library search first; enhance rather than recreate it. |
| Multi-file batch queue | ACCEPT | M16 queue / M | Reuse the existing job lifecycle with bounded inference concurrency. |
| Global speaker directory | DEFER | M18 gated directory / M | Start with manual identity links; automatic voice matching is not committed. |
| Bilingual dual-subtitles | ACCEPT-ADAPTED | M15 endpoint path / M; M17 local path | Translation comes only from the opt-in Polish endpoint or a future local SLM. |
| Styled ASS export | ACCEPT-ADAPTED | M15 basic ASS / S; M18 karaoke gate | Basic styles first; karaoke tags require validated word timing. |
| Clean/verbatim export | ACCEPT-ADAPTED | M15 derived export / S | Conservative previewed cleanup; no unconditional deletion of meaningful Indonesian words. |
| Quote cards/audiograms | DEFER | M18 media gates / S then L | PNG feasibility precedes video encoding; neither blocks core transcription. |
| Indonesian slang normalization | ACCEPT-ADAPTED | M15 text rules / M | Opt-in, reversible suggestions; slang is not inherently an error. |
| Local SLM Polish | ACCEPT-ADAPTED | M17 runtime/model gates / L | Optional downloads and measured EN+ID quality precede release. |
| Meeting notes/action items | ACCEPT-ADAPTED | M17 structured outputs / M | Endpoint or validated local SLM; unsupported owners/deadlines stay unspecified. |
| Local diarization | ACCEPT-ADAPTED | M16 gated engine slices / L | Blocked on M10A sherpa dylib packaging and model/runtime validation. |
| Custom vocabulary | ACCEPT-ADAPTED | M15 glossary / M | Small token-budgeted Whisper prompt plus a separate deterministic correction dictionary. |
| Silero VAD/chunked transcription | ACCEPT-ADAPTED | M16 capture pipeline / M | Pause-delimited chunks, not a promise of cloud-style realtime latency. |
| Acoustic event tags | DEFER | M18 classifier gate / L | Requires a separately pinned classifier and false-positive evaluation. |
| Android voice-note share target | ACCEPT-ADAPTED | M13 JNI, import+FGS jobs, touch Deck slices / M | Refines the already-planned `ACTION_SEND`; compact Deck presentation, not another UI stack. |
| Android foreground recording | ACCEPT-ADAPTED | M13 import+FGS jobs and mic slices / M | Strengthens existing lifecycle work; no guarantee against all OS termination. |
| Android Quick Settings tile | DEFER | M18 Android tile / S | Useful later; not necessary for the Android launch. |

No whole candidate needs outright rejection. Rejected implementations and commercial patterns appear in §6.

## 3. New milestone definitions

### M15 — Transcript Workbench

**Goal:** Make imported transcripts easier to correct, search, and export without adding a continuously resident inference service.

**Existing dependencies:** import/decode pipeline, transcript persistence, segment editor, playback, current library search, export serializers, Model Manager, and the opt-in Polish integration. Inspect actual schemas and APIs before migrations; the workorder names below do not assert existing repository paths.

#### Ordered workorders

1. **M15.1 — Persistence and provenance audit · S** — deliver a schema/API inventory covering source text, segment mutations, derived text, search indexing, and alignment metadata; specify additive migrations only where needed.
   **Verify:** upgrade a populated v0.3.0 fixture, exercise backup/restore, demonstrate no source-text loss; record which search capabilities already exist.
2. **M15.2 — Import parity and shared decode reuse · S** — desktop Ogg/Opus/WebM import already ships (M10C: `symphonia-adapter-libopus`, bundled libopus); deliver fixture parity checks across the desktop and Android paths and a shared decode-interface review so Android reuses the same code. No new decoder; no external `ffmpeg`. (The research suggested `audiopus`/`lewton` — replaced by what the repo already ships.)
   **Verify:** shared fixtures (webm, mkv-opus, raw Ogg-Opus) decode identically on both paths; malformed/truncated containers, varying input rates, cancellation.
3. **M15.3 — Transactional split/merge · M** — platform-appropriate shortcuts, undo, deterministic text partitioning, time-boundary validation; invalidate or recompute derived alignment/translation when source segments change.
   **Verify:** boundary splits, merges, undo/reopen, consistent search results, unchanged unrelated segments.
4. **M15.4 — Subtitle diagnostics and basic ASS · S** — advisory CPL/CPS badges and a basic ASS serializer from the corrected segment model; configurable 37/42 CPL and 17/20 CPS warning levels; document counting rules.
   **Verify:** Unicode/grapheme cases, multiline text, zero-duration cues, long Indonesian words, ASS escaping, playback in an independent reader.
5. **M15.5 — Small glossary and reversible text rules · M** — bounded glossary prompt builder plus a separate post-correction dictionary; respect the actual Whisper context budget (~224 tokens, subject to the integrated version and other prompt use) with visible truncation feedback; optional Indonesian normalization and conservative clean-export previews.
   **Verify:** prompt budgeting, word-boundary rules, EN+ID code-switching, preservation of terms such as "kayak" when semantically meaningful. Prompt biasing is not advertised as WER elimination.
6. **M15.6 — Alignment feasibility, then playback UI · M + M** — first establish which current engine bindings expose usable token timing and how it maps to words; then persist supported alignment and add click-to-seek/highlighting.
   **Verify:** against reviewed EN+ID fixtures, publish alignment-error results and failure cases; fall back to segment seeking when unavailable or stale; reduced motion removes spring effects without removing playback state.
7. **M15.7 — Search enhancement · M** — timestamp-bearing snippets and seek actions; add or improve FTS5 only after M15.1; Unicode-friendly baseline (English stemming must not damage Indonesian retrieval).
   **Verify:** insert/edit/delete synchronization, migration backfill, mixed-language queries, literal Indonesian terms, large-corpus responsiveness, missing-media behavior.
8. **M15.8 — Opt-in bilingual export · M** — explicitly requested segment translation, editable secondary text, stacked SRT/VTT/basic ASS output; store translation provenance and mark translations stale after source edits.
   **Verify:** EN→ID and ID→EN, cue alignment, escaping, endpoint failure/cancellation, zero requests while endpoint use is disabled; users without an endpoint retain ordinary export.

**Non-goals:** system-wide dictation, new STT engines, automatic translation without consent, guaranteed word precision, animated caption rendering, full video editing, a promise of linguistically perfect cleanup.

### M16 — Capture & Speakers

**Goal:** Add useful desktop capture and session-local speaker attribution without entangling capture permissions with diarization readiness.

**Existing dependencies:** M15 segment-edit invariants, desktop audio capture, `suramak-taps`, existing inference job management, Model Manager. Diarization additionally requires completed **M10A sherpa release packaging**.

#### Ordered workorders

1. **M16.1 — Background job lifecycle · M** — bounded multi-file queue with per-item state, cancellation, errors, progress; serialize access where an inference context requires it.
   **Verify:** mixed successful/failed imports, cancellation, shutdown, bounded memory, UI responsiveness.
2. **M16.2 — Capture/HUD lifecycle · M** — explicit idle, recording, processing, result, cancellation, permission-failure states in a floating Deck surface; warmed models optional and measured distinctly from idle targets.
   **Verify:** rapid start/stop, device removal, long captures, keyboard access, motion preferences, no hidden recording.
3. **M16.3 — Hotkey and insertion adapters · M per platform** — independently releasable macOS/Windows/Linux integrations; probe platform permissions and compositor support; manual copy when insertion is unavailable; no privileged `uinput`, no clipboard inspection as a universal solution.
   **Verify:** focus changes, permission denial/revocation, shortcut conflicts, insertion failure, clipboard behavior disclosed.
4. **M16.4 — VAD chunking · M** — pinned Silero model, worker-side chunking with bounded buffers, cancellation, overlapping-context handling; audio callbacks only enqueue bounded data.
   **Verify:** silence, short utterances, noisy EN+ID code-switching, dropped/duplicated text, CPU use, end-to-end latency on named hardware. Whisper remains the Indonesian fallback.
5. **M16.5 — Additional desktop loopback · M per platform** — retain macOS `suramak-taps` with regression coverage; Linux monitor-source selection and Windows WASAPI loopback as separate workorders after their feasibility checks.
   **Verify:** device changes, absent monitor sources, permissions, timestamp drift, recording-state visibility; loopback and microphone capture are distinct (mixed capture not implied).
6. **M16.6 — Diarization packaging gate, inference, integration · S + M + M** — first verify M10A packaged artifacts and supported model interfaces; then integrate one validated offline diarization pipeline with pinned models; finally persist session-local speaker turns and render names/colors with manual correction.
   **Verify:** clean-machine dynamic-library loading, corrupt/missing model handling, EN+ID multi-speaker fixtures, overlap limitations, speaker-count errors, cancellation, transcript alignment. A failed packaging gate blocks these slices, not unrelated capture work.
7. **M16.7 — Cached waveform and boundary editing · M** — bounded multiresolution waveform cache, draggable segment boundaries using M15 transaction semantics.
   **Verify:** long recordings, cache eviction, playback synchronization, undo, invalid-boundary rejection.

**Non-goals:** realtime diarization guarantees, known-person recognition, cross-session biometric matching, universal Linux text injection, meeting bots, automatic recording on application detection, requiring a warm inference model at login.

### M17 — Local Polish

**Goal:** Offer optional offline refinement and structured notes while preserving source evidence and the existing endpoint choice.

**Existing dependencies:** Model Manager, Polish request/result handling, M15 derived-text provenance, optional M16 speaker labels (they improve notes; they do not block them).

#### Ordered workorders

1. **M17.1 — Runtime and model feasibility · M** — evaluate one runtime direction at a time (`llama.cpp` bindings or `candle`) with a small candidate model set; review licenses, EN+ID behavior, memory, cancellation, package size, platform support.
   **Verify:** reproducible measurements on named machines, including a representative 4 GB Android device before any Android commitment. Stop if a useful quality/resource envelope cannot be established.
2. **M17.2 — Optional local Polish backend · M** — pinned downloads, explicit backend selection, bounded loading, cancellation, unload behavior; unsupported platforms retain the opt-in endpoint path.
   **Verify:** offline operation after download, download corruption/resume, out-of-memory failure, no automatic endpoint fallback.
3. **M17.3 — Refinement and local translation · M** — previewed text refinement and, only if bilingual evaluation passes, the local translation source for M15 exports.
   **Verify:** names, numbers, negation, Indonesian register, code-switching, hallucination rates; failed translation evaluation leaves local translation unavailable rather than silently substituting another service.
4. **M17.4 — Structured meeting notes · M** — summaries, decisions, action items through the chosen backend, with transcript references and explicit "unspecified" owner/deadline values; transcript instructions are data, not commands.
   **Verify:** unsupported claims, invented tasks, malformed structured output, long-input limits, correction without modifying the transcript.

**Non-goals:** autonomous task execution, cloud fallback, guaranteed factual summaries, mandatory model downloads, shipping all researched SLMs/runtimes.

### M18 — Optional Media Tools

**Goal:** Validate secondary capabilities individually, without making them prerequisites for core transcription.

**Existing dependencies:** M15 export/alignment infrastructure, M16 speaker metadata where applicable, M17 model-evaluation practices, the shipped Android recording lifecycle.

#### Gated workorders

1. **M18.1 — Manual speaker directory · M** — user-created identities and explicit links from session-local speakers.
   **Verify:** rename/unlink/delete behavior, no automatic identity inference; voice-embedding storage stays outside this approval.
2. **M18.2 — Quote card, then audiogram gate · S + M** — selected-text PNG export using Deck styling; separately investigate native video encoding, licensing, packaging, and resource cost before committing an audiogram.
   **Verify:** Unicode rendering, explicit output location, no hidden upload, cancellation. Karaoke ASS proceeds only if M15 alignment quality supports it.
3. **M18.3 — Acoustic-event feasibility · M** — evaluate one pinned classifier against reviewed speech/noise fixtures before any production integration.
   **Verify:** per-class precision/recall, false tags on Indonesian speech, compute cost, fully removable derived annotations. "Silence" is not equated with missing speech.
4. **M18.4 — Android Quick Settings tile · S** — reuse the existing foreground recording state and notification controls.
   **Verify:** locked-device behavior, permission denial, duplicate starts, current Android background-start restrictions.

**Non-goals:** full NLE functionality, cloned voices/TTS, automatic global voice identification, hardware microphone-array diarization, any obligation to ship failed experiments.

## 4. Sequencing & rationale

Finish platform launches before expanding platform-specific capture. M15 then delivers broad owner value with limited runtime risk. M16 splits capture from its packaging-blocked diarization branch. M17 introduces another model runtime only after download, provenance, and cancellation conventions are established. M18 remains optional.

```text
v0.3.0 shipped
   |
   +--> M13 Android launch ----+
   |                          |
   +--> M14 Windows launch ---+--> M15 Transcript Workbench
                                      |
                                      +--> M16 Capture & Speakers
                                      |       ^
M10A sherpa release packaging --------+ diarization gate
                                      |
                                      +--> M17 Local Polish
                                                |
M15 exports/alignment + M16 speakers ------------+--> M18 optional gates
```

Default order: M13 → M14 → M15 → M16 → M17. Existing launch work may continue independently; no shared decoder change should destabilize a launch. If capacity tightens, retain M15 editing, diagnostics, glossary, and search before alignment polish; within M16, ship verified capture adapters independently — do not delay them solely for diarization. Release claims must describe actual platform support, not roadmap intent.

## 5. Deliberately not copied

- **Cloud-mandatory STT, meeting bots, telemetry, crash uploaders:** incompatible with sovereignty and trust.
- **Webviews or framework migrations:** competitor architecture does not justify replacing `egui`/`wgpu`; OpenCut's GPUI port is early.
- **Subscription gating, proprietary-only exports, store dependence:** retain user ownership, GPL-3.0 distribution, open exports, GitHub releases.
- **Unbounded glossary claims:** Scribe's 1,000-keyterm mechanism does not transfer to Whisper.
- **Destructive cleanup and identity claims:** slang, fillers, and speaker clusters require context and user control.
- **Unsupported legal/performance marketing:** no automatic compliance guarantees, fabricated precision, or unmeasured latency claims. (Aiko's verified price is $24 one-time, not $9.99 — pricing does not drive this roadmap.)

## 6. Open questions for the owner

1. Approve M15's workbench-first scope, with system-wide dictation moved to M16?
2. Which desktop platform should receive the first fully verified hotkey/insertion adapter?
3. Which existing workorder owns the M10A sherpa release packaging, and which artifacts must clear its gate?
4. With Android Auto Backup ON, which transcript/recording/model/credential categories should be included or excluded?
5. Should M18 remain an unscheduled experiment pool until M17 quality and resource measurements are reviewed?
