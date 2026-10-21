# Windows smoke — first run (M14b)

The first time Suramak runs on Windows. Everything here is **owner-only**: CI proves
the binary compiles, links, carries its resources and passes the unit tests
(`docs/reports/M14b.md` §4), but nothing on the runner can launch a window.

## 0. What you need

- Windows 11 x64 (the target is Windows 11 only).
- The two CI artifacts from the **newest green run** of
  <https://github.com/LoLyeah/suramak/actions/workflows/ci.yml>:
  - `suramak-gui-windows-x86_64` → `suramak.exe` (the app)
  - `suramak-cli-windows-x86_64` → `suramak-cli.exe` + `dumpbin-dependents.txt`
- No installer yet (M14g). The exe is standalone: static CRT, no VC++ redistributable.

Unzip both into one folder, e.g. `C:\Users\Han\Desktop\suramak-test\`.

## 1. The launch checklist (M14b's acceptance)

Run `suramak.exe` by double-clicking.

| # | Check | Expected |
|---|---|---|
| 1 | Console window | **None.** A GUI-subsystem binary must not flash or keep a console. |
| 2 | Icon | The Suramak icon in the title bar and in the taskbar while running. |
| 3 | Version properties | Right-click `suramak.exe` → Properties → Details: `File version` and `Product version` filled (not blank, not `0.0.0.0`). |
| 4 | Window | Opaque Deck surface (no acrylic yet — that is M14c). Resize, maximize, snap (Win+←/→). |
| 5 | Scaling | At 100% and 150% display scaling the text stays crisp; dragging to a second monitor with a different scale does not blur or shrink the UI. |
| 6 | **Single instance** | With the app running, launch `suramak.exe` again. The **first** window comes to the front; **no second window** appears; the second process exits (Task Manager shows one `suramak.exe`). |
| 7 | Drag-in | Drag an audio/video file onto the window (or use Import if drag-in is not wired yet) — the file lands in the library. |
| 8 | Reveal | Settings → Recordings folder row → **Show in Explorer** opens Explorer with that folder selected. |
| 9 | Data location | `%LOCALAPPDATA%\Suramak\` contains `suramak.db`, `settings.toml`, `models\`, `recordings\`. Nothing under `%APPDATA%`. |

Report #1–#9 with pass/fail; a screenshot of the window + one of the Properties → Details
dialog is enough evidence.

## 2. The pipeline on Windows (the real test)

The engine and the pipeline are the same code as macOS/Linux, but this is their first
execution on Windows. In the app:

1. **Model** — Settings → Models → download `Tiny (fastest)` (or `Large v3 Turbo (balanced)`
   if you want the default quality). Watch that the download completes and the sha256
   check passes.
2. **Import** — import a short English clip, transcribe. Expected: the editor opens with
   timestamped segments.
3. **Import Indonesian** — same with `id` selected as the language. Indonesian must go
   through whisper (there is no Parakeet on Windows).
4. **Edit + export** — edit a segment, export Markdown, open the file.
5. **Record** — record ~10 s from the microphone, Stop, wait for the transcript
   (`RECORD_AUDIO`-equivalent: Windows shows the microphone privacy prompt the first time).
6. **Playback** — play the recording, click the waveform to seek.

## 3. CLI checks (PowerShell, same folder)

```powershell
.\suramak-cli.exe doctor
.\suramak-cli.exe models list
.\suramak-cli.exe transcribe .\some-clip.mp3 --model tiny --lang en
.\suramak-cli.exe transcribe .\some-clip-id.mp3 --model small --lang id
```

Expected: `doctor` reports the platform/engine block with no errors; both transcribes
print segments and write the export files next to the input (or under `--out`).

## 4. Known gaps — do not file these as bugs (yet)

- **No acrylic/DWM material** — the window is opaque until M14c.
- **No notifications / taskbar progress** — M14f.
- **No installer, no updater, no signing** — M14g. Windows SmartScreen will warn on an
  unsigned exe: *More info → Run anyway*.
- **No GUI log** — the `windows_subsystem` attribute means stdout/stderr are invisible.
  If something fails silently, run the CLI: it prints to the console.
- **Polish key storage** — Credential Manager wiring is M14d. Do **not** test polish on
  Windows yet; the key may not persist.
- **Parakeet / SpeechTranscriber** — neither exists on Windows (Parakeet packaging is its
  own workorder; SpeechTranscriber is Apple-only).

## 5. What to send back

For each numbered check: pass/fail + one line. For failures: the exact action, what you
saw, and (if the app is still running) Task Manager → `suramak.exe` → right-click →
*Create dump file* is **not** needed yet — a screenshot and the steps are enough at this
stage. Anything that looks like a crash: check `Event Viewer → Windows Logs → Application`
for an `Application Error` entry naming `suramak.exe` and paste its faulting module.

## 6. Findings from the owner's first run (2026-10-21) — to fix, in order

From the first screenshots of the shell on the owner's PC. None block testing; all are
mac-tuned-constants/copy gaps the Windows bring-up must absorb:

| # | Symptom | Cause (hypothesis) | Fix slice |
|---|---|---|---|
| 1 | Settings shows **"System follows the macOS light/dark setting live"** on Windows | theme helper text is not cfg-per-platform | copy seam, M14c |
| 2 | **The floating Record pill overlaps the "Automatic updates" checkbox** (covers "new versions in the background") | the scroll region/pill constants were tuned on macOS fonts; the pill-covers-no-button invariant (USER_SMOKE §20) needs Windows-space verification | layout pass, M14e |
| 3 | The **"Appearance" heading is clipped** at the top of the Settings scroll | same mac-tuned vertical rhythm | layout pass, M14e |
| 4 | **"← Home" was not discoverable in the editor** — the owner got stuck in the first transcript; the link IS present top-left of the editor pane (confirmed in the second pass screenshot) | so: not absent, just unnoticed mouse-first (a quiet top-left text link, no button chrome) | make it a real (still quiet) control so mousing works — M14e; owner already knows Esc works (keymap §24) |
| 5 | **Progress bar sits at 0% until the job completes** (seen on a 9 s clip; the job view's "← Home" link was present here, refining #4) | not a wiring defect — the callback chain (whisper progress cb → trampoline → UI) is correct and test-pinned; balance: whisper.cpp fires progress once per finished encode window (~30 s of audio), so short files produce only 0→100 | UX polish: interpolate the bar between Progress events (elapsed-time estimate over the audio duration) so short jobs show motion; long jobs already tick — motion polish, M14e |

Also noted: the Models manager is intentionally **not** in Settings (Home → Models, since
M3), and there is no system-audio source on Windows until **M16** (Windows loopback +
per-app capture) — both are documented scope, not bugs.

## 8. Item 5 result (second instance) — guard works; raise unverified

The owner ran a second `suramak.exe` while the first was up: **no second window, no
second process** ✓ — the D2 guard's mutex path works on real Windows. Expected beyond
"refuse": the SECOND process signals and **the first window raises to the front**
(`AllowSetForegroundWindow` + auto-reset event → `UserEvent::Activate` →
restore/focus). The owner's wording ("it refuses to open") suggests the raise did not
visibly happen — unconfirmed whether the first window was already foreground at the
moment of the test. To settle on the next run: make sure another app is focused first,
then launch the second instance and watch whether Suramak jumps forward.

---

## 7. First-run verdict (2026-10-21 evening) — **PASS**

The owner's run on the Windows PC, build `suramak-gui` @ `b46a0ff` (`39593916027`):

- **Checklist items 1–8 pass**: no console flash · icon in title bar + taskbar · version properties filled · resize/snap · **multi-monitor + DPI-different displays: fine** (owner-tested) · single instance untested at that moment (item 5) · EN clip (Tiny, `en`): **correct transcription** matching the CI fixture ("The quick brown fox…") · ID clip (Tiny, `id`): **correct transcription**, 3 timed segments · mic recording: done, library row with correct metadata · models downloaded via Home → Models (the manager works; it is **not** a Settings section by design) · "Show in Explorer" present.
- **Engine/kubernetes honesty at the edges**: `engine: CPU` (no GPU path by scope — Vulkan follows), **Polish dimmed** with the exact reason + the M8 no-console caveat — the §24/§31 degradation copy renders correctly on Windows.
- **Real waveform** verified on Windows (cover §23's invariant): envelope matches speech, gap bands, click-to-seek, readout ticks.
- **Logged findings**: §6 items 1–5 above (2 layout/copy bugs + pill-overlap; polish + progress-bar interpolation + Home discoverability assigned to M14c/M14e).
- **Deliberately untested**: polish end-to-end (Credential Manager is M14d), notifications/taskbar progress (M14f), installer/updater (M14g).

This closes the first real-hardware Windows pass — the M14a/M14b lane is verified beyond CI's reach.
