# M21 — the improvement roadmap (Carbon high + competitor research)

Source: `gemini-4-carbon` (thinking 12288), 2026-11-12, briefed with the
product's verified truth and a competitor pass (MacWhisper, Anti-Vocale,
Google Recorder/Live Transcribe, Whistle, Offscript, Otter/Fireflies/Notta).
Raw answer below; the parent verified each initiative against the repo and
marked scope conflicts.

## Competitor matrix (what they have that we do not, Nov 2026)

| Competitor | Good ideas worth adopting | Do NOT copy |
|---|---|---|
| **MacWhisper** (macOS) | batch + **watched folders**; **bot-free system-audio meeting capture**; integrations (Obsidian/Notion/webhooks); **CLI**; DOCX/PDF | App-store pricing split; cloud AI defaults |
| **Anti-Vocale** (Android, Apache-2.0) | **automatic voice-note transcription** (WhatsApp/Telegram), **paginated transcript notification**, per-app settings, **Open Transcribe contract**, auto-save to a chosen folder, engine choice (Whisper/Parakeet/Nemotron/Qwen3), custom model import | **media-folder scraping / NotificationListener** — breaks our privacy story; the honest path is Share/SAF/Open-Transcribe |
| **Google Recorder / Live Transcribe** | **search across all recordings**; **sound-event tags**; live captions | Pixel-only engine lock-in |
| **Whistle** | record+transcribe with the screen locked; auto language detect | ads |
| **Offscript** | encrypted store (Keystore) + **biometric lock**; tags; calendar; 4 h recordings | heavy vector DB |
| **Otter / Fireflies / Notta** | speaker-labelled live notes, **summaries/minutes**, action items | cloud ASR, calendar auto-join bots, accounts |

## Carbon's 8, ordered (verbatim in the appendix)

1. **Persistent batch queue + segment checkpointing** (M) — survives an OS
   kill mid-lecture; resumes at the last committed segment.
2. **Zero-detour voice-note notification** (Android, S) — share-target →
   headless FGS → paginated, copyable notification transcript.
3. **Cross-library full-text search + richer rows** (M) — search every
   transcript, land on the word; mini-waveforms/speaker pips in rows.
4. **On-device voiceprint memory** (S) — ⚠ see the scope question below.
5. **Bot-free dual-bus meeting capture (macOS)** (L) — system audio + mic on
   separate buses; "You" by construction; remote voices diarized.
6. **Watched inbox / Markdown vault dropzone** (S) — a folder in, transcripts
   out (Speckle-free, mtime-settle gated).
7. **Citation-verified polish views** (S) — Minutes / Action items / EN↔ID
   brief as non-destructive sidecars; every `[mm:ss]` validated against real
   segments, fake ones stripped.
8. **Shared-queue CLI + `Open Transcribe` contract** (M) — one queue, one
   model, no duplicate RAM.

## Parent verification

- #1 ✓ the queue is in-memory (`queue/driver.rs`); checkpointing is new work.
  The persistence layer (SQLite/WAL) already exists to hang it on.
- #2 ✓ share-target + FGS + notification already ship (M13F/M13G2); the
  paginated transcript notification is new.
- #3 ✓ today's search is per-transcript; library-wide FTS is new. The M15
  plan recorded "FTS5 only after an audit" — that audit is the gate.
- #4 ❌ **DECLINED (owner, 2026-11-12): "No — keep it manual naming only."**
  No voiceprint gallery, no stored CAM++ centroids, no cross-recording
  suggestion chips. Manual naming (M16I/M18.1) is the whole feature; any
  future revisit starts from an explicit owner request.
- #5 ✓ `suramak-taps` already captures system audio on macOS; the dual-bus +
  You-labelling is the addition. Carbon's gate (ScreenCaptureKit drift
  < 40 ms over 60 min) replaces the taps path only if it proves better —
  taps stays the baseline.
- #6 ✓ nothing exists; `notify` would be a named dependency cost (~40 KB,
  OS-native events) — needs the owner's nod or a polling fallback.
- #7 ✓ polish exists (M11); the sidecar presets + citation validation are new
  and cheap.
- #8 ✓ the CLI and its queue integration exist; the Android intent contract
  is new.

## Sequencing

- **v0.5 (next)**: #1 checkpointed queue, #2 voice-note notification, #3
  library-wide FTS (+ the richer rows that fall out of it).
- **v0.6**: #6 watched inbox, #7 citation-verified polish views, #8 Open
  Transcribe.
- **Gated before code**: #5 dual-bus drift spike; the EN+ID "campur-code"
  dual-pass idea (B1 in the appendix) needs the 25-clip WER gate. #4 is
  declined and out of the roadmap.

---
*Epistemic tags: `(KNOWN)` = directly from brief; `(INFERRED)` = engineering, OS, or market deduction.*

### A. The 8 Highest-Value Improvements (Ordered by Leverage)

1. **Persistent Batch Queue & Segment Checkpointing**
   - **Pitch:** Upgrade the one-slot job queue `(KNOWN)` into a persistent FIFO batch queue that flushes completed segments to disk incrementally.
   - **User job:** Drop a week of macOS interviews at once, or survive an Android OS background kill mid-lecture in Jakarta without restarting from `00:00` `(INFERRED)`.
   - **Effort / Risk:** **M** | Cancellation/resume state races; zero new dependencies using append-only JSONL checkpoints `(INFERRED)`.
   - **Acceptance test:** Force-kill Suramak at 50% of a 60-minute transcription; relaunch resumes within 2s of the last committed segment with zero duplicated or missing text.

2. **Zero-Detour Voice-Note Notification (Android)**
   - **Pitch:** Route Share-target imports into headless FGS transcription `(KNOWN)` via a 1.2s frosted confirmation sheet, posting a paginated, copyable notification transcript `(KNOWN)`.
   - **User job:** Read long Indonesian or English WhatsApp/Telegram voice notes `(KNOWN)` on noisy commutes without leaving the chat app `(INFERRED)`.
   - **Effort / Risk:** **S** | OEM notification height clamping truncating long speaker turns `(INFERRED)`.
   - **Acceptance test:** Share a 2-minute `.opus` voice note from WhatsApp; focus returns to WhatsApp in <1.5s and tapping `Copy` on the finished notification copies full glossary-corrected text.

3. **Cross-Library Full-Text Search & Rich Row Anatomy**
   - **Pitch:** Index all transcript segments for instant timestamp jump-search while upgrading barebones Library rows `(KNOWN)` with mini-waveforms, 8px speaker-colour pills, and Geist Mono durations `(KNOWN)`.
   - **User job:** Find one specific EN/ID phrase across 200 recordings on macOS or Android and land directly on that word in the Editor `(KNOWN)`.
   - **Effort / Risk:** **M** | Index drift after split/merge edits; use a tiny pure-Rust inverted index to avoid heavy C dependencies `(INFERRED)`.
   - **Acceptance test:** Search a word spoken at `42:10` across 50 saved files; clicking the snippet opens Editor seeked to `42:10` within the `FAST` motion budget `(KNOWN)`.

4. **On-Device Voiceprint Memory (`CAM++` Gallery)**
   - **Pitch:** When a user manually renames a speaker `(KNOWN)`, optionally persist their `CAM++` embedding centroid `(KNOWN)` locally to pre-suggest names in future recordings.
   - **User job:** Stop re-labelling recurring teammates, family, or bilingual interview subjects across sessions `(INFERRED)`.
   - **Effort / Risk:** **S** | False positives on short/noisy clips—mitigate via honest `Rina?` suggestion chips requiring 1-tap confirmation `(INFERRED)`.
   - **Acceptance test:** Rename a speaker in File A; transcribe separate File B offline with the same voice and verify their badge offers a confirmable `Rina?` chip.

5. **Bot-Free Dual-Bus Meeting Capture (macOS)**
   - **Pitch:** Capture macOS system audio alongside mic input on separate channels behind a compact AppKit Liquid Glass pill `(KNOWN)`, pairing deterministic "You" labelling with `CAM++` remote diarization `(KNOWN)`.
   - **User job:** Record Zoom/Teams calls on our primary macOS platform `(KNOWN)` without inviting cloud bots `(KNOWN)`.
   - **Effort / Risk:** **L** | `ScreenCaptureKit` permissions and Bluetooth AirPods sample-rate drift `(INFERRED)`.
   - **Acceptance test:** Record a 45-minute headphones call; mic speech maps 100% to `You` with <40ms clock drift, while remote participants diarize cleanly on channel 2.

6. **Watched Inbox & Markdown Vault Dropzone**
   - **Pitch:** Watch a user-selected folder for new audio, enqueue automatically, and export speaker-badged `.md`/`.srt` files `(KNOWN)` into an Obsidian or Syncthing folder `(KNOWN)`.
   - **User job:** Hands-free pipeline from hardware recorders or phone-synced folders into local knowledge vaults `(INFERRED)`.
   - **Effort / Risk:** **S** | Reading half-synced files; gate enqueueing behind mtime/size settle checks using lightweight `notify` (named cost: ~40 KB source, OS-native fs events) `(INFERRED)`.
   - **Acceptance test:** Copy a 100 MB `.m4a` into the Inbox; Suramak waits for write completion, transcribes, and writes a valid `.md` file with zero partial-read errors.

7. **Citation-Verified Polish Views (Minutes & EN/ID Briefs)**
   - **Pitch:** Add non-destructive Newsreader-typeset `(KNOWN)` sidecar presets (Diarized Minutes, Action Items, EN↔ID Brief) to the opt-in OpenAI-compatible endpoint `(KNOWN)`, tuned for localhost Ollama `(INFERRED)`.
   - **User job:** Distill raw bilingual meetings into readable minutes without mutating the verbatim transcript.
   - **Effort / Risk:** **S** | LLMs hallucinating timestamps; validate every `[mm:ss]` tag against real Editor segments to uphold strict honesty `(KNOWN)`.
   - **Acceptance test:** Feed a mock polish response containing real `[04:12]` and fake `[99:00]`; UI renders `[04:12]` as a working seek chip and strips `[99:00]`.

8. **Shared-Queue CLI & `Open Transcribe` Contract**
   - **Pitch:** Ship a desktop `suramak` CLI and Android's `Open Transcribe` intent `(KNOWN)` that hand jobs to our existing queue, models, and glossary `(KNOWN)`.
   - **User job:** Automate Raycast/Shortcuts scripts, Tasker, or Forkgram `(KNOWN)` without loading duplicate models into RAM `(INFERRED)`.
   - **Effort / Risk:** **M** | IPC lock contention when GUI and CLI run simultaneously `(INFERRED)`.
   - **Acceptance test:** Run `suramak clip.ogg --json` while the GUI is mid-job; CLI job queues behind the active job with zero duplicate model allocation.

---

### B. Three "Signature" Mechanisms Competitors Cannot Cheaply Copy

1. **"Campur-Code" EN+ID Dual-Pass Workbench:** Indonesian professionals constantly mix English terms into Bahasa sentences (`"tolong follow-up deck-nya"`) `(INFERRED)`. Parakeet is EN-only while Indonesian requires Whisper `(KNOWN)`, which often mangles English loanwords phonetically `(INFERRED)`. **Mechanism:** Pass 1 runs Whisper (ID) seeded with the user's Glossary `(KNOWN)` in `initial_prompt`. A token-probability scanner isolates low-confidence sub-2s audio spans in the decoded PCM buffer and runs fast `Parakeet`/`SpeechTranscriber` `(KNOWN)` *only* on those slices in RAM, surfacing 1-tap dual word-chip alternatives (`[deck | dek]`) backed by existing Editor undo `(KNOWN)`.
2. **Bot-Free "Glass Room" Diarized Minutes (macOS):** Webview wrappers burn hundreds of MBs before ML loads `(INFERRED)` and cloud tools inject bots `(KNOWN)`. **Mechanism:** A tiny Liquid Glass pill `(KNOWN)` taps `ScreenCaptureKit` (remote bus) + CoreAudio mic (local bus) `(INFERRED)`. Physical channel isolation yields 100% accurate `You` attribution with zero ML compute, focusing `pyannote + CAM++` `(KNOWN)` + Voiceprint Memory (#4) strictly on remote voices, finishing into Newsreader-typeset sidecar minutes (#7).
3. **Zero-Cloud Voice-Note-to-Quote-Card Loop (Android):** **Mechanism:** Share a WhatsApp `.opus` note -> FGS runs `whisper.cpp` + `CAM++` offline `(KNOWN)` -> notification offers a 1-tap `Share Quote PNG` action that rasterizes selected segments using bundled Newsreader/Inter/Geist Mono and speaker badge colours `(KNOWN)` into a crisp card ready to send right back into the group chat—zero cloud, zero webview.

---

### C. What NOT to Copy (and Naive Honesty Traps)

- **Cloud ASR, Accounts, & Calendar Auto-Join Bots (Otter/Fireflies — `KNOWN`):** Directly violates our zero-cloud, zero-account promise `(KNOWN)` and drags in OAuth/network polling `(INFERRED)`.
- **Naive "Zero-Tap" WhatsApp Media Scraping (Anti-Vocale trap — `KNOWN`/`INFERRED`):** On Android 11+, auto-watching WhatsApp's private media folder requires broad `MANAGE_EXTERNAL_STORAGE` or `NotificationListenerService` access to *every* notification `(INFERRED)`. Adopting that naively breaks our privacy story. **Honest path:** Explicit Share-sheet, `Open Transcribe`, or user-chosen SAF folders.
- **Fake "Streaming" Whisper Captions (MacWhisper/Recorder trap — `KNOWN`/`INFERRED`):** Whisper's 30s window `(INFERRED)` causes flickering retroactive rewrites when forced into live mode, violating our rule to never fake state `(KNOWN)`. Only offer live text via true streaming engines (`SpeechTranscriber` / streaming `sherpa-onnx` — `KNOWN`/`INFERRED`) with visually distinct muted "draft" vs. solid "committed" chips.
- **Local Vector DBs & DOCX/PDF Crates (Offscript/MacWhisper — `KNOWN`):** Bundling ~100 MB embedding models and brittle layout crates violates our named-dependency-cost constraint `(KNOWN)` when exact FTS + Markdown/PNG cover real needs `(INFERRED)`.

---

### D. Android-First UX: Making the Phone App Feel Loved

- **Voice-Note Inbox:** A `Voice Notes` filter chip in Library auto-collecting Share-target clips; shows source app or sender *only* when confirmable in intent metadata—never guessed `(KNOWN)`.
- **Notification Transcript:** Paginated notification showing real decode-ahead progress % `(KNOWN)`, speaker prefixes, `Next Page`, `Copy`, and `Open Editor`.
- **QS Tile Flows:** Tapping the QS tile `(KNOWN)` starts FGS mic capture immediately *without* booting the full `wgpu` Activity `(INFERRED)`, posting lock-screen `Pause`, `Bookmark`, and `Stop & Transcribe` controls.
- **Share-Sheet Behaviour:** Painted-frost micro-sheet `(KNOWN)` with `ID / EN` pill and press-spring physics `(KNOWN)` that auto-dismisses in 1.2s (`SLOW` token), keeping the user inside chat.
- **Offline / Airplane States:** Zero warning banners offline—show a calm Geist Mono `ON-DEVICE • OFFLINE` pill; gracefully pause resumable model downloads `(KNOWN)` and mark opt-in Polish jobs `Waiting for network`.
- **Long-Recording Resilience (4h+):** Record FGS audio as chunked Ogg/WAV flushed via `fsync` every 5s `(INFERRED)`—avoiding unfinalized MP4 `moov` corruption on battery death `(INFERRED)`—paired with segment checkpointing (#1).
- **OEM Battery-Saver Quirks:** Log an honest "FGS started vs. cleanly exited" sentinel in Diagnostics `(KNOWN)`. Only after an unexpected OEM kill (Xiaomi/Samsung/Oppo — `INFERRED`) or when queuing >30m files, surface a plain-spoken OEM whitelist card.

---

### E. Sequencing & Feasibility Gates

- **Next Release (`v0.5` — Flow & Visual Richness):** **#1** Persistent Batch Queue, **#2** Android Voice-Note Notification + Chunked 4h Recording, **#3** Cross-Library FTS + Rich Row Anatomy (completing the UI refresh `(KNOWN)`).
- **After the Next (`v0.6` — Memory & Automation):** **#4** `CAM++` Voiceprints, **#6** Watched Inbox/Dropzone, **#7** Citation-Verified Polish Views, **#8** CLI + `Open Transcribe`.
- **Gated Before Any Code:**
  1. *#5 / Glass Room Dual-Bus (macOS)* — **Gate:** 150-LOC Rust spike measuring `ScreenCaptureKit` + Bluetooth HFP mic drift over 60m `(INFERRED)`; must stay <40ms glitch-free.
  2. *Campur-Code Dual-Pass (B1)* — **Gate:** Offline benchmark on 25 real EN/ID code-switched clips; must cut English-loanword WER ≥15% vs. Whisper+Glossary alone at <25% latency cost `(INFERRED)`.
  3. *`wgpu`-Free Android FGS (D)* — **Gate:** Verify Android lifecycle runs `whisper.cpp` + `CAM++` inside an FGS while the `egui` surface is destroyed, without Vulkan/OOM crashes on 6 GB phones `(INFERRED)`.

---

### F. How the Moat Compounds

Cloud tools rent commodity APIs and trap transcripts behind logins; Electron/webview apps burn gigabytes of RAM just to draw buttons `(INFERRED)`. Because Suramak is native Rust (`egui` + `wgpu` — `KNOWN`), its tiny UI footprint leaves maximum memory headroom on laptops and mid-range Android phones for simultaneous `whisper.cpp`, `Parakeet`, and `CAM++` `(KNOWN)`. Every recording compounds locally: voiceprints get sharper, EN/ID glossaries stop repeating mistakes, and the FTS archive becomes an instant personal memory—creating a private, tactile instrument that cloud bots are neither fast enough nor trusted enough to replace.