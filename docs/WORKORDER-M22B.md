# WORKORDER — M22B: the voice-note notification (Android)

Roadmap: `docs/reports/M21-roadmap.md` #2 (Carbon high, adapted from
Anti-Vocale — without their media-scraping trap). Today a shared voice note
opens the app and runs a visible job; the user waits. This slice keeps them
in the chat and hands the transcript back through the notification.

## Deliverables

1. **Headless share path**: `ACTION_SEND` audio → a 1.2 s frosted micro-sheet
   (Sheet plane, ID/EN pill, press-spring buttons) offering "Transcribe" (or
   auto-starts when the language is unambiguous and a model is verified) →
   the FGS runs the existing job in the background. The Activity may stay
   behind or close; the job must not depend on the egui surface.
2. **The transcript notification**: a `MessagingStyle`/`BigTextStyle`
   notification (the API that supports paging) that shows the CLEANED
   transcript as it grows, with actions: **Copy**, **Next page**
   (long transcripts), **Open editor**, **Share quote card** (the M18.2
   renderer). Speaker prefixes appear when diarization ran (M16I); the
   sender's name only when the sharing intent actually provided it — never
   guessed.
3. **Progress honesty**: the same notification shows the decode-ahead
   percent the queue reports; no fake "streaming" text (the model is not
   streaming — the UI says "Transcribing… 62%").
4. **The end state**: on completion the notification updates to "Transcript
   ready · 1,240 words" with the actions above; tapping opens the editor at
   the top. On failure: the named reason from the queue, plus Retry.
5. **No new permissions, no NotificationListener, no media-folder access.**
   The share intent is the only ingest; say so in the report.

## Verification

- Host tests: the notification builder's text paging (a 5,000-word transcript
  page count and the Copy payload = the full cleaned text); the intent→language
  pill mapping; the "sender only when provided" rule; the failure copy.
- The emulator smoke gains a step: `am start` a share intent with the fixture
  wav, assert the notification exists (`dumpsys notification`) and its text
  contains the first segment's words, then Cancel. (No model download needed:
  the fixture path uses the tiny model already installed? If not, gate the
  step behind a model check and document.)
- fmt/clippy -D warnings; NDK cross-check (this crate is Android-only);
  workspace tests green (one full run at the end).
- Report: `docs/reports/M22b-voice-note-notification.md`.

## Do NOT

- No WhatsApp/Telegram scraping, no NotificationListener, no
  MANAGE_EXTERNAL_STORAGE; no second transcription path (the queue stays the
  one); no new crates; no CI edits beyond the smoke step; no git commit/push;
  no sudo.
