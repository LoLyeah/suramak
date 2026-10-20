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
