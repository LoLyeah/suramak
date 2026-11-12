# M22B — the voice-note notification

Status: **implemented and host-tested.** The emulator step exists but its
notification half is gated on a verified speech model (a fresh `-wipe-data`
emulator has none); the micro-sheet half runs ungated. No permissions were
added.

## 1. What the user gets

Sharing an audio file from a chat opens Suramak on the compact Library with a
**236 pt frosted micro-sheet** (the Sheet plane, the drag handle, the M20A
press-spring on every body):

```
                 ─────
  Ptt 2026-11-12 at 09.14.opus            Cancel
  Rina                                    ← only when the intent carried it
  Language                    [ ID | EN ]
  On this phone · nothing is uploaded.
  [              Transcribe              ]
  ▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔ (1.2 s countdown)
```

* **Auto-start** happens only when the language is unambiguous — an explicit
  `language` extra on the share, or a fixed language in Settings — **and** a
  verified model is installed. The sheet then dismisses itself at the design's
  `SLOW` token (1.2 s), the job is scheduled, and the user can stay in the chat.
  The thin accent line above the title is that window, made visible.
* **Otherwise** the sheet waits: the user picks ID/EN (tapping the selected
  pill again returns it to Auto) or taps Transcribe with Auto. With no model,
  Transcribe is disabled and the state line says exactly that.
* **Dismissing** (Cancel or BACK) starts nothing and drops the pending share.

The job is the existing M13E/M16.1 path: `TranscriptionJobService` copies the
URI, the one `QueueDriver` decodes and transcribes, the store write is the only
transcript write. The only addition is `kind = voice_note` (plus the sender) on
the job record, so the notification surface can be chosen for the whole run —
including a job resumed from the journal after a process death.

The Activity is **not** required for the work. While a voice-note job is live,
BACK at the Library root becomes `moveTaskToBack`, not `finishAndRemoveTask`:
the task goes behind (back to the chat) and the foreground service keeps the
queue alive in the same process. A process the system actually destroys pauses
the job through the existing journal, and the next launch offers Resume. The
job never calls into egui or the compact app.

## 2. The notification

**Live, every 5 % (the queue's own integer):**

```
Voice note
Transcribing… 62%                         [ Cancel ]
```

There is deliberately **no partial transcript**: whisper decodes 30-second
windows and any "growing" text would be a retroactive rewrite pretending to be
a stream. The copy is generated in Rust (`voice_note::progress_text`) so the
phone cannot drift from the tested sentence.

**Finished (`BigTextStyle`, the paging API):**

```
Transcript ready · 1,240 words
Page 3 of 56
Rina                                       ← only if the intent named one
  Speaker 1: …the page’s text…
  Speaker 2: …
[ Copy ] [ Next page ] [ Open editor ] [ Share quote card ]
```

* **Copy** writes `copy_text` — the **whole cleaned transcript**, with speaker
  prefixes, never the page. The notification then shows `Copied · Page 3 of 56`
  so the tap is visible without a toast.
* **Next page** wraps (`page % page_count`), re-posts in place.
* **Open editor** is the existing M13G deep link (Library → Editor).
* **Share quote card** renders the current page at 1080 px wide (Deck tokens:
  Inter SemiBold title, Newsreader body, Geist Mono footer), stages one PNG
  under `files/exports/quotes/`, and hands it to the share chooser through the
  app's FileProvider. The staging is pruned at the next process start, the
  same policy the export staging uses.
* **Speaker prefixes** appear only when a diarization run exists for the row;
  the label is the stored one (`Speaker 1` or the owner's manual name).
* **The sender** is the notification's subtext only when the intent provided a
  name. There is no contact lookup, no filename-as-name, and no guessing.
* **Failure:** `Transcription failed` with the queue's named reason, plus
  **Retry**. Retry re-queues the same copied media (failure now keeps it) and
  schedules the foreground pump again; a refusal is posted verbatim.

## 3. Ingest and privacy

* The **share intent is the only ingest**. `ACTION_SEND` / `ACTION_SEND_MULTIPLE`
  with an audio mime enters the micro-sheet; video keeps the direct import.
* **No NotificationListenerService, no MANAGE_EXTERNAL_STORAGE, no media-folder
  scan, no new permission.** The manifest change is one non-exported receiver
  for Copy / Next page / Retry and two actions on the existing Activity.
* The sender is only `app.suramak.extra.SENDER`, or `Intent.EXTRA_TITLE` when
  that value is not the media's display name. The Rust rule
  (`sender_from_intent`) trims and rejects empty; it never derives a name.
* The language pill only accepts `id`, `in` (legacy spelling), `en` and their
  region variants. An explicit tag we do not understand (`fr`) stays on Auto —
  it never falls back to the app's configured language, because that would
  silently transcribe a French note as Indonesian.

## 4. Paging and the clean pass

* `PAGE_WORDS = 90` words per page. A 5,000-word transcript is
  `ceil(5000/90) = 56` pages; every page is ≤ 90 words, pages concatenate
  back to the copy payload in order, and page indices wrap.
* The **clean** pass is `textrules::preview` with `collapse_whitespace` only:
  trim + single spaces. The dictionary and the Indonesian normalizer are
  **not** applied silently — the stored text and its source stay untouched.
* Speaker prefixes are added at the page/line boundary; a segment longer than
  a page keeps its prefix only on its first chunk.

## 5. Files

| File | What it is |
|---|---|
| `crates/suramak-android/src/voice_note.rs` | Pure offer plan, progress/failure copy, paging + notification model, quote-card staging. 12 host tests. |
| `crates/suramak-ui/src/deck/quote_card.rs` | The M18.2-shaped pure card renderer (text + metadata + theme → pixels), 3 tests. |
| `crates/suramak-ui/src/deck/compact/voice_note.rs` | The micro-sheet UI (Sheet plane, ID/EN pill, press-spring, 1.2 s countdown), 2 tests. |
| `crates/suramak-ui/src/deck/compact/model.rs` | `Sheet::VoiceNote`, `VoiceNoteView`, `CompactAction::VoiceNoteAccept/Dismiss`, the clock. |
| `crates/suramak-ui/src/deck/compact/chrome.rs` | `micro_sheet` (the sheet material at the micro height). |
| `crates/suramak-android/src/jobs.rs` | `kind`/`sender` on the job record, `Status.current_kind`, `Status.last_failed`, `retry`, `voice_note`/`quote_card` ops, failure keeps the copy. |
| `crates/suramak-android/src/compact_host.rs` | `voice_note_offer` op + `sheet` snapshot value. |
| `crates/suramak-android/src/shell.rs` | Accept/dismiss intents, BACK-cancel flag, `voice note sheet: shown` marker. |
| `android/.../transcribe/Notify.kt` | `voiceNoteProgress`, `voiceNoteReady`, `voiceNoteFailed`, the four actions, `openTranscript`. |
| `android/.../transcribe/VoiceNoteActionReceiver.kt` | Copy / Next page / Retry (no Activity). |
| `android/.../transcribe/TranscriptionJobService.kt` | `kind`/`sender` extras, voice-note progress and completion, job-key diagnostic line. |
| `android/.../spike/MainActivity.kt` | share → offer, accept → enqueue, quote-card chooser, BACK keep-alive. |
| `scripts/android/emulator-smoke.sh` | The M22B step and its model gate. |

## 6. Verification

### Host tests (all green)

```
voice_note (suramak-android lib)              13 passed
voice_note offer + sheet (compact host)       1 passed
voice_note notification / retry (jobs host)   2 passed
quote_card (suramak-ui lib)                   3 passed
voice_note sheet auto-dismiss (suramak-ui)    2 passed
voice-note sheet integration (phone suite)    3 passed
```

The two tests the workorder names are exact:

* `a_five_thousand_word_transcript_pages_and_the_copy_payload_is_the_whole_text`
  — 5,000 words → 56 pages; every page ≤ 90 words; the pages together are the
  same 5,000 words in order; `copy_text` is the full cleaned text, not page 1.
* `the_language_pill_maps_intent_tags_and_never_guesses` plus
  `the_offer_auto_starts_only_with_a_language_and_a_verified_model` — the
  intent→pill rule and the auto-start gate.
* `the_sender_exists_only_when_the_intent_provided_one`.
* `failure_copy_names_the_queue_reason_and_never_invents_one`.
* `speaker_prefixes_appear_only_when_diarization_ran`.
* `the_quote_card_stages_a_real_png_and_cleans_up`.

Integration with the real store and the real decoder:
`a_voice_note_notification_is_built_from_the_stored_row` (kind + sender survive
the queue → store → notification model → staged PNG) and
`a_failed_voice_note_keeps_its_copy_and_can_be_retried` (failure keeps the
media, names the reason, retry re-queues).

### Static / cross checks

* `cargo fmt` on every touched file — clean.
* `cargo clippy -p suramak-android --all-targets -- -D warnings` — clean.
* `cargo clippy -p suramak-android-spike --all-targets -- -D warnings` — clean.
* `cargo ndk -t arm64-v8a check -p suramak-android` — clean.
* `cargo ndk -t arm64-v8a clippy -p suramak-android -p suramak-android-spike --all-targets -- -D warnings` — clean.
* Kotlin: `./gradlew :app:compileDebugKotlin` — `BUILD SUCCESSFUL` (the manifest
  merge and the new receiver included).
* Workspace tests: one full run at the end of the slice — **1,279 passed, 0
  failed, 11 ignored**. The baseline was 1,236; this slice adds 24 tests (21
  host + 3 phone-viewport integration). The tree also carried the parallel
  M22A slice, so the delta is not all M22B's.

### Emulator smoke

`scripts/android/emulator-smoke.sh` gains one step after the model-truth
assertion:

1. Push the committed `fixtures/audio/en.wav` (and a 90 s loop of it) into the
   app's external files dir; push a model only when
   `SURAMAK_SMOKE_MODEL=/path/to/ggml-tiny.bin` is set.
2. `am start -a android.intent.action.SEND -t audio/wav --eu … --es language en`
   — assert the shell's `voice note sheet: shown` marker.
3. **No model:** print `M22B-SKIP` naming the gate, press BACK, assert the
   offer was dismissed and nothing was scheduled. CI takes this path.
4. **Model present:** run 1 with the 90 s loop — wait for the
   `Transcribing…` notification, take the job key from the service's own
   `voice-note job key=…` line, send the same Cancel broadcast the notification
   uses, assert `Transcription cancelled`. Run 2 with the 9 s fixture — assert
   `Transcript ready` and that the body contains `quick brown fox` (the first
   segment's words), as `dumpsys notification --noredact` reports it.

The gate is honest: the notification half is not exercised without a model, and
the script says so instead of passing vacuously. To run it locally, point
`SURAMAK_SMOKE_MODEL` at a `ggml-tiny.bin` (the same file the desktop uses).

## 7. Limits and decisions

* **The in-process FGS stays in the Activity's process.** A voice-note job is
  headless in the sense that it never touches egui; it is not in a second OS
  process. BACK keeps the task (and process) alive for the run; a system kill
  pauses and the journal resumes, exactly as M13E already did. A second
  process would duplicate the job journal and the library, so it was not taken.
* **Retry is bounded by the process lifetime**: failed records are in memory,
  not the journal. The copy is kept for Retry and deleted when the record is
  pruned (16 terminal records) or discarded; a process death loses the failed
  record and leaves one copy for the next prune window. This is named rather
  than hidden.
* **BigTextStyle, not MessagingStyle.** Both support paging; BigText gives one
  reliable body and a summary line (`Page 3 of 56`) across OEMs. MessagingStyle
  bubbles would shrink a 90-word page on several launchers.
* **The card is the M18.2 shape, not the M18.2 milestone.** `WORKORDER-M18B`
  was never implemented; this slice builds only the renderer the notification
  action needs (title, attribution, page body, footer, light/dark, 1080 px) and
  reuses the deterministic host rasteriser. The full M18.2 selection UI and its
  golden set remain separate work.
* **The quote card is dark-themed** because the notification is a system
  surface with no appearance signal; a future action can pass the app's
  resolved theme.
* **No emulator run was performed in this workspace** (no KVM/AVD); the CI step
  is the emulator gate, and its model half stays gated until a model is
  provisioned.

## 8. The privacy sentence, stated once

The share intent is the only ingest. Suramak does not read WhatsApp/Telegram
media folders, does not run a NotificationListenerService, does not hold
MANAGE_EXTERNAL_STORAGE, and adds no permission for this feature. Everything
above happens on the phone; the notification is the transcript coming back to
the chat, not a copy leaving the device.
