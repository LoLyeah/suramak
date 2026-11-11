# M16.5-F — Linux monitor-source + Windows WASAPI loopback: feasibility

**Decisions: Linux GO (slice 0 is a PipeWire gate) · Windows GO (slice 0 is the owner's-PC
probe). Neither platform needs a new crate.** Both conclusions are backed by a real
capture on this VPS (Linux) or by the pinned `cpal` source (Windows); no production code,
no dependency, and no CI file was touched.

The M6 mixer contract (16 kHz mono PCM16, per-source gain, drift correction) is met by
both paths **as long as the capture layer delivers contiguous audio-time**; Windows
loopback does not do that by itself when nothing is playing (§4, §7 — the one real
integration hazard).

---

## 1. Machine-named numbers

```text
probe_date=2026-11-11
probe_host=VM-0-5-ubuntu
probe_cpu=AMD EPYC 7K62, 2 vCPU
pulseaudio_version=17.0
cpal_version=0.15.3
alsa_version=1.2.15.3
libasound2_plugins_version=1.2.12-2build1

linux_host_count=1
linux_host_ids=Alsa
linux_input_device_count=2
linux_input_device_names=default,pulse
linux_input_config_default=2ch@44100Hz/F32
linux_input_config_pulse=2ch@44100Hz/F32
linux_monitor_name=probe.monitor
linux_monitor_addressable_by_name=false
linux_monitor_open_error=no-such-input-device

linux_cpal_monitor_wall_s_30s=30.000
linux_cpal_monitor_frames_30s=1258524
linux_cpal_monitor_rate_hz_30s=44100
linux_cpal_monitor_channels_30s=2
linux_cpal_monitor_format_30s=F32
linux_cpal_monitor_audio_s_30s=28.538
linux_cpal_monitor_rtf_30s=0.9513
linux_cpal_monitor_rms_30s=0.17676
linux_cpal_monitor_peak_30s=0.24997
linux_cpal_monitor_zcr_hz_30s=440.0
linux_cpal_monitor_stream_errors_30s=0

linux_parec_monitor_bytes_30s=914482
linux_parec_monitor_audio_s_30s=28.578
linux_parec_monitor_rtf_30s=0.9520
linux_parec_monitor_rms_30s=0.17676
linux_parec_monitor_zcr_hz_30s=440.0

linux_cpal_rtf_10s_silent_a=0.9437
linux_cpal_rtf_10s_silent_b=0.9837
linux_cpal_env_scope_monitor_rms=0.17676
linux_cpal_env_scope_monitor_zcr_hz=440.0
linux_cpal_env_scope_default_rms=0.00000
linux_cpal_default_monitor_macro_rms_3s=0.17674
linux_cpal_pulse_pcm_macro_rms_5s=0.17676

windows_loopback_in_cpal=true
windows_loopback_flag_source=cpal-0.15.3/src/host/wasapi/device.rs:566-572
windows_render_default_input_config=Err(StreamTypeNotSupported) [device.rs:526-531]
windows_render_supported_input_configs=empty [device.rs:465-475]
windows_render_mix_format_source=device.rs:534-541
windows_capture_wait_mode=INFINITE [stream.rs:238-241]
windows_loopback_silence_packets=false [no synthesis in stream.rs:374-430]
windows_mix_format_24bit_supported=false [device.rs:203-229]
windows_new_packages_for_loopback=0
windows_crate_version=0.54.0
windows_crate_bytes=11096265
windows_crate_license=MIT OR Apache-2.0
windows_wasapi_crate_occurrences_in_lock=0
```

The 4.8 % deficit in `linux_cpal_monitor_rtf_30s` is **the virtual null sink's clock, not
cpal**: `parec` (an independent libpulse client) recorded `0.9520` over the same wall
window — 0.14 % more audio than cpal (0.07 RTF points). The virtual sink's ratio is not stable run to run
(`0.9437` and `0.9837` on two 10 s silent runs, no cpal change); a hardware monitor source
is clocked by the output device, i.e. the sink's clock is the DAC clock (±~100 ppm).

---

## 2. What the Linux probe actually ran

No audio server and no sound card exist on this VPS (`/proc/asound/cards` absent,
`/dev/snd` contains only `seq`/`timer`). The probe therefore started a user-mode
PulseAudio with a virtual sink — the smallest dummy that exposes a real `*.monitor`
source:

```bash
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
     pulseaudio pulseaudio-utils                 # 1:17.0+dfsg1-2ubuntu4 — the only sudo used
pulseaudio --start --exit-idle-time=-1           # user daemon, socket /run/user/1000/pulse/native
pactl load-module module-null-sink sink_name=probe \
     sink_properties=device.description=ProbeSink      # source probe.monitor, s16le 2ch 44100Hz
pactl set-default-source probe.monitor
pactl unload-module module-suspend-on-idle       # 30 s window; re-loaded for the idle test (§3.5)
```

`libasound2-plugins` 1.2.12-2build1 (the ALSA `pulse` PCM) was already installed; the apt
line above is the only package change, and the only sudo (§10).

Signal: `/tmp/m16g/tone440.wav`, a 45 s **440 Hz** sine at 0.25 full scale, 44.1 kHz
stereo s16, played with `paplay -d probe`. Capture in the same 30 s wall window by two
independent clients: `parec -d probe.monitor --rate=16000 --channels=1 --format=s16le`
and the throwaway probe (`cpal =0.15.3`, release) through cpal's ALSA `default` device.
The 440 Hz zero-crossing estimate from both is **440.0 Hz**, so the monitor path carries
the actual sink audio, not silence or a synthetic stream.

The throwaway crate lives at `build/m16g-probe/` (git-ignored: `git check-ignore` resolves
`.gitignore:13:build/`), built with `CARGO_TARGET_DIR=/tmp/m16g-target`. Modes:
`list`, `try <name>`, `open <name> <secs>`, `dual <monitor> <secs>`, `loopback <name> <secs>`
(the last one is the Windows-pass shape; it compiles on Linux and was smoke-run).

Probe hygiene: `pulseaudio -k` after the measurements; the daemon was not running before.

---

## 3. Linux: cpal cannot *name* a monitor, but it can capture one

**The API surface.** cpal 0.15.3 has exactly one Linux host: ALSA
(`cpal-0.15.3/src/host/mod.rs:1-8`; JACK only behind the `jack` feature). There is no
PulseAudio host and no PipeWire host in the crate — the answer to "pulse host or pipewire
host?" is **neither**. Measured on this box:

- `cpal::available_hosts() = [Alsa]`, `default_host().id().name() = "ALSA"`.
- `host.input_devices()` = **`["default", "pulse"]`**, both `2ch@44100Hz/F32`.
- `try probe.monitor` → `found=false error=no-such-input-device`.
- `Settings` picker (`list_input_devices_detailed`, `capture.rs:498-513`) would therefore
  list exactly **"default"** and **"pulse"** — never a monitor, on any PulseAudio box.

**But the capture works.** `cpal` opens the ALSA `pulse` PCM (the libasound PulseAudio
plugin), and that plugin honours **`PULSE_SOURCE`** when it connects the record stream.
With `PULSE_SOURCE` naming the monitor, the 30 s cpal read above delivered 1 258 524
frames at 44.1 kHz — **28.538 s of audio in 30.000 s, RTF 0.9513** — containing the
440 Hz tone at RMS 0.17676, peak 0.24997. `parec` in the same window measured RTF 0.9520:
the deficit is the virtual sink's clock, identical for both clients.

**The env var is read per open, not per process.** `dual` opened two cpal streams in one
process: with `PULSE_SOURCE=probe.monitor` set only around the first
`build_input_stream`, then removed before the second open (default source deliberately
pointed at a *different*, silent sink):

```text
stream=monitor  frames=289897  rms=0.17676  zcr_hz=440.0   # tone
stream=default  frames=345714  rms=0.00000  zcr_hz=0.0     # silent sink
```

So a single process can hold a monitor stream and a microphone stream at once, as long as
the monitor stream is opened while the variable is set and the variable is cleared before
any other audio client opens. That is the whole Linux design, and it needs **no new
dependency**.

**No server introspection is needed either.** `PULSE_SOURCE=@DEFAULT_MONITOR@` resolves:
via ALSA `default` (RMS 0.17674, 440.2 Hz) and via ALSA `pulse` (RMS 0.17676, 440.0 Hz).
That token means "monitor of the current default sink", which is exactly the
"record what the computer plays" source. It is the primary name; a concrete
`<default-sink>.monitor` from `pactl get-default-sink` is the fallback (`pactl` is only
*Suggested* by `pipewire-pulse`, so it must not be the primary path).

**Device-name strategy, stated exactly.** Host = cpal ALSA (the only one). Device =
the ALSA PCM named **`pulse`** (fallback `default` only if `pulse` is absent — on a
PipeWire desktop `default` is the native `pipewire` plugin, which is not a PulseAudio
client and cannot see `PULSE_SOURCE`). Source = **`PULSE_SOURCE=@DEFAULT_MONITOR@`**,
scoped to that one open. Picker = no monitor rows; "System audio" is a source *type*,
which is already the shape of the macOS taps path.

### Linux hazards, measured or named

1. **`PULSE_SOURCE` is process-global.** `std::env::set_var` is safe API in edition 2021
   but is unsound against concurrent `getenv` (Rust 2024 makes it `unsafe`). The app must
   serialise *all* audio opens behind one mutex, set → open → restore with a drop guard,
   and open no other *capture* stream in that window (playback is unaffected — `PULSE_SINK`
   is the separate variable).
2. **PipeWire is unverified.** Ubuntu 24.04+ and Fedora default to PipeWire. The
   `pulse` PCM (libasound2-plugins → pipewire-pulse) is the portable bet, and
   pipewire-pulse implements the PulseAudio protocol, but whether it resolves
   `@DEFAULT_MONITOR@` here was not tested (no PipeWire on this VPS). This is slice 0.
3. **The picker cannot show monitors.** A monitor is a source name, not an ALSA device.
   Do not build a monitor device list on Linux; expose "System audio" and let the server
   choose.
4. **Virtual sinks blow past the mixer's ±2 % drift clamp** (`mix.rs:80-82`): 0.9513 is
   −48 700 ppm, so `MixOutcome.reports[].clamped` is `true`. Real sinks are DAC-clocked;
   do not validate drift on a null sink.
5. **Idle sinks keep ticking (measured).** With `module-suspend-on-idle` loaded and
   nothing playing, a 10 s monitor read still delivered 9.837 s (RTF 0.9837, RMS 0.0) —
   PulseAudio keeps a monitored sink running. No gap-fill is needed on Linux, but the
   production code should log an RTF < 0.9 as a source problem.

---

## 4. Windows: cpal 0.15.3 ships WASAPI loopback — no addition needed

**Answered from the pinned source, not docs.** cpal 0.15.3 (the only cpal in
`Cargo.lock:1052-1055`) implements loopback in its WASAPI backend:

- `cpal-0.15.3/src/host/wasapi/mod.rs:17-20` documents it: *"If you use a WASAPI output
  device as an input device it will transparently enable loopback mode."*
- `cpal-0.15.3/src/host/wasapi/device.rs:566-572`, inside `build_input_stream_raw_inner`:
  ```rust
  let mut stream_flags = Audio::AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
  if self.data_flow() == Audio::eRender {
      stream_flags |= Audio::AUDCLNT_STREAMFLAGS_LOOPBACK;
  }
  ```
  The flag is decided by the **endpoint's** data flow, so any render endpoint opened as
  an input stream is a loopback stream. No `wasapi` crate is involved: cpal 0.15 talks to
  WASAPI through `windows 0.54.0` (`cpal` `Cargo.toml:169-183`, features
  `Win32_Media_Audio`, `Win32_System_Com_StructuredStorage`, …); `Cargo.lock` contains
  **zero** `wasapi` packages.

**The API shape the capture path must use.** `default_input_config()` on a render
endpoint returns `Err(StreamTypeNotSupported)` (`device.rs:526-531`) and
`supported_input_configs()` is empty (`device.rs:465-475`) — so today's
`capture.rs:628` (`device.default_input_config()`) can never open loopback. The working
shape is:

```text
device  = host.default_output_device()            // or a chosen render endpoint
support = device.default_output_config()          // the render mix format, device.rs:534-541
config  = support.config(); format = support.sample_format()
stream  = device.build_input_stream_raw(&config, format, …)   // device.rs:81-99
```

`build_input_stream` (typed) works too, because it forwards to `build_input_stream_raw`
with `T::FORMAT` (`traits.rs:125-150`) and never re-checks the device's input configs.
`host.devices()` enumerates `eAll` active endpoints (`device.rs:877`), so output devices
already appear in the picker — with an empty input-config list, which is why the UI must
special-case them rather than show them as microphones.

**Smallest addition if cpal could not do it: zero new packages.** `windows 0.54.0` is
already compiled for every Windows build through cpal (`Cargo.lock:5699-5702`), with the
`Win32_Media_Audio` features a wrapper needs; a direct
`windows = { version = "0.54", default-features = false, features = ["Win32_Media_Audio",
"Win32_System_Com"] }` would unify with that copy. The crate is **11 096 265 B**
(MIT OR Apache-2.0). A wrapper would add source lines under `cfg(windows)`, not a
dependency, and it is **not needed**: the loopback flag is already in the pinned cpal.

### Windows hazards the owner's-PC pass must settle

1. **No packets while nothing renders.** WASAPI loopback only produces data while another
   stream is rendering on the endpoint (Microsoft's loopback-capture sample; PortAudio issue
   #935 reports the same on Win7/Win10). cpal's capture loop returns
   `ControlFlow::Continue` on zero available packets (`stream.rs:385-387`) and its wait is
   `INFINITE` on the endpoint event (`stream.rs:238-241`) — it never synthesises silence.
   The M6 drift measurement would then see `N < rate·D` and clamp the correction at −2 %,
   so the gap becomes a fixed time offset for every later word (the clamp prevents a
   catastrophic stretch; it does not restore alignment). **Fix in the capture layer: use
   `InputCallbackInfo` timestamps and insert exact zero frames for gaps**, or hold a
   silent render stream on the same endpoint. This is the one design decision the
   Windows slice must make explicitly.
2. **Event-driven loopback is version-sensitive.** cpal always requests
   `AUDCLNT_STREAMFLAGS_EVENTCALLBACK | LOOPBACK` and waits `INFINITE`
   (`device.rs:566-572`, `stream.rs:238-241`). Older Windows documentation says the
   data-ready event never fires for loopback (the classic sample uses a waitable timer).
   cpal loopback does work for users on Win10/11, but this is the first thing to measure
   on the owner's PC: callbacks must arrive within ~2 s of a tone starting.
3. **24-bit mix formats are invisible to cpal.** `format_from_waveformatex_ptr`
   (`device.rs:203-229`) maps only 8/16/32/64-bit PCM and 32-bit float; a 24-bit
   `GetMixFormat` returns `None`, `default_output_config()` errors, and loopback cannot
   open at all. Consumer endpoints usually expose 32-bit float, but the owner's DAC may
   not. The pass records the mix format before anything else.
4. **Endpoint changes kill the stream.** `AUDCLNT_E_DEVICE_INVALIDATED` is mapped to
   `BuildStreamError::DeviceNotAvailable` (`device.rs:555-558`); switching to headphones
   mid-session needs a re-open on the new default render endpoint.
5. **Endpoint loopback only.** cpal captures the whole endpoint, not one app. Per-app
   capture (the M16 "session-audio" precedent) needs
   `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK` (Win10 2004+) and is out of scope here;
   it matches the macOS split (taps for system audio, no arbitrary per-app on Windows).

---

## 5. M6 mixer contract — both platforms meet it, with one Windows condition

The contract is in `crates/suramak-asr/src/mix.rs` and the capture tail:

| Requirement | Mechanism (cited) | Linux | Windows |
|---|---|---|---|
| 16 kHz mono PCM16 output | `TARGET_RATE = 16_000` (`core/resample.rs:7`); rubato `FftFixedIn` 1024/2, exact truncation (`core/resample.rs:16-48`); `write_recording_wav` → `write_wav_i16` (`capture.rs:96-101`) | yes | yes |
| Rate conversion exists for device rates | `resample_16k` (`capture.rs:79-86`) = core's rubato path; stage 1 of `mix` (`mix.rs:159`). 44.1 kHz measured here; 48 kHz is the same code path | yes (44.1 k) | yes (48 k typical) |
| Mono downmix before the mixer | `capture_rt::MonoFrames` in the RT callback (`capture.rs:62-73`); `to_16khz_mono` averages channels too | yes (2 ch) | yes (2 ch) |
| Per-source gain | `MixTrack.gain`, applied before the sum (`mix.rs:87-97`, `183-186`) | yes | yes |
| Drift correction | ρ = N/(rate·D), Catmull-Rom stretch, clamp ±2 % (`mix.rs:80-82`, `175-181`) | yes, but a **virtual** sink clamps (−48 700 ppm measured) | yes **iff gaps are filled**; otherwise N/D is wrong |
| Non-zero rate required | `mix` rejects `rate == 0` (`mix.rs:144-151`) | 44 100 | 48 000 (mix format) |

The only contract gap is not in the mixer: **Windows must deliver contiguous audio-time.**
Filling WASAPI silence gaps with exact zero frames (or holding a silent render stream)
keeps `N/(rate·D)` equal to the endpoint clock, which is what the drift stage assumes.
Linux needs no equivalent: a monitored sink keeps ticking in silence (measured
`linux_cpal_rtf_10s_silent_b = 0.9837`, RMS 0.0).

---

## 6. Decision — Linux

**GO.** Zero new dependencies; real capture proven on PulseAudio 17; the mechanism is
`PULSE_SOURCE=@DEFAULT_MONITOR@` on the ALSA `pulse` PCM, scoped to one open.

Production slices, each independently stoppable:

| # | Slice | Gate before the next |
|---|---|---|
| 0 | **PipeWire desktop probe** (named hardware, not this VPS): Ubuntu 24.04+/Fedora GNOME, PipeWire 1.x. Confirm the ALSA `pulse` PCM exists, `@DEFAULT_MONITOR@` resolves through pipewire-pulse, and a 30 s monitor read has RTF in [0.99, 1.01] with the tone intact | If it fails: try `<default-sink>.monitor` from `pactl get-default-sink`; if that also fails, stop and take the libpulse-simple decision (`libpulse-simple-binding 2.29.0`, 13 697 B, MIT OR Apache-2.0; libpulse 17 LGPL-2.1 already on target desktops) |
| 1 | **Linux SystemAudio open**: `system_audio_available()` becomes true on Linux (`capture.rs:191-193`), new branch opens ALSA `pulse` with `PULSE_SOURCE=@DEFAULT_MONITOR@` under a process-wide audio-open mutex + drop guard; env restored on every error path | mic + monitor open simultaneously; monitor stream carries the tone, mic stream does not; no env leak after an induced open failure |
| 2 | **Recorder wiring**: `Recorder.device_name` = "System audio (monitor)", rate from the plugin config, into `MultiRecorder` | a 10-minute mic + monitor session; both sources present in `MixOutcome.reports` |
| 3 | **Settings/UI**: "System audio" row as a source type (no monitor device row); disabled with the reason when the `pulse` PCM is absent | picker shows 2 devices + the system-audio row; no crash when the server is absent |
| 4 | **Telemetry**: log per-source RTF and `clamped` at stop | a suspended-sink run and a normal run are distinguishable in the log |

**Re-visit condition:** if slice 0 fails on PipeWire *and* the libpulse-simple dependency
is rejected, Linux system audio goes **DEFER** and the platform ships mic-only (today's
state; `system_audio_available()` stays false).

---

## 7. Decision — Windows

**GO.** cpal 0.15.3 already sets `AUDCLNT_STREAMFLAGS_LOOPBACK` for render endpoints;
zero new packages; endpoint loopback is exactly the "system audio" scope the plan
accepted (M16's system-audio item is accepted-adapted, so this is the measured path, not
a new promise).

Production slices:

| # | Slice | Gate before the next |
|---|---|---|
| 0 | **Owner's-PC probe** with the throwaway binary (§8): mix format (bits, rate, channels), tone, 60 s RTF, silence-gap packet count, device switch, 24-bit endpoint if present | 440 Hz ± 2 Hz captured; RTF 60 s recorded; gap behaviour recorded; mix format known |
| 1 | **Windows SystemAudio open**: `CaptureInput::SystemAudio` → `default_output_device()` + `default_output_config()` + `build_input_stream_raw` (never `default_input_config`) | tone capture; rate/channels reported from the mix format |
| 2 | **Gap policy**: timestamp-driven zero-fill (or a silent keep-alive render stream, if slice 0 shows the timestamps cannot be trusted); unit-test the fill against a synthetic gap | 15 s of silence mid-session leaves N/D within 0.1 % of wall; mixer `drift_ppm` ≈ 0 |
| 3 | **Endpoint-change recovery**: `DeviceNotAvailable` → re-open default render endpoint or stop with a warning | unplug/replug headphones mid-session |
| 4 | **24-bit decision**, only if slice 0 finds a 24-bit mix format: direct `windows 0.54` wrapper (0 new packages, MIT OR Apache-2.0) converting S24→f32; otherwise nothing | capture on that endpoint |
| 5 | **Mixer integration + telemetry**: 48 kHz stereo → mono → 16 kHz; `MixTrack { rate: 48_000 }`; per-source gain, clamped reporting | 10-minute mic + system session on the owner's PC |

**Re-visit condition:** if slice 0 gets no callbacks within 5 s of a playing tone (the
event-mode hazard), do not chase cpal: implement the capture loop on the same
`windows 0.54` pin with a waitable timer (still 0 new packages), or **STOP** Windows
system audio and ship mic-only.

---

## 8. The Windows pass, for the owner

One throwaway binary, already written and compiled on Linux (`build/m16g-probe`,
git-ignored, `cpal =0.15.3` only; the cpal API it uses is platform-neutral):

```powershell
cd build\m16g-probe
cargo run --release -- list
cargo run --release -- loopback "Speakers" 60      # tone must be playing
```

`list` prints the host, every input device with its default input config, and the
output-device names (it is how you get the exact name to pass to `loopback`). `loopback` opens the named **output** endpoint as an input stream using
`default_output_config()` + `build_input_stream_raw` — the exact shape slice 1 will use —
and prints `channels/rate/sample_format/frames/callbacks/audio_s/rtf/rms/peak/zcr_hz/errors`.
Run it three times: silent, 440 Hz tone playing, and with playback stopped 15 s into a
60 s run. Pass criteria: tone → `zcr_hz` 440 ± 2 and `rms > 0`; `rtf` within 0.999–1.001
for the 60 s tone run; the stopped run shows `audio_s < wall_s` by the silence span (the
gap that slice 2 must fill). Then check the endpoint's mix format under
Sound → Properties → Advanced; a 24-bit format is the one case that needs slice 4.

The pass runs on **named hardware** — the owner's Windows PC. No emulator or VM was used
here, and none of the Windows numbers above is a runtime measurement.

---

## 9. What this report does not establish

- No hardware monitor source was captured: this VPS has no sound card, so the Linux
  numbers come from a PulseAudio null sink (real reads, virtual clock).
- PipeWire was not installed or tested; the `pulse` PCM path is the design bet, gated by
  Linux slice 0.
- No Windows binary was run: the loopback answer is source-inspected from the pinned
  cpal and grounded in its issue history; runtime facts wait for §8.
- 24-bit endpoints, device hot-plug, and per-app capture are not measured on any OS.
- The `0.9513` Linux RTF is a property of the probe environment, not of cpal, and must
  not be quoted as a hardware capture figure.

---

## 10. Verification of this work order

- **No production code, no dependency, no CI change.** `git status --porcelain` was clean
  before this report; the only added file is this one. The probe lives under
  `build/m16g-probe/`, ignored by `.gitignore:13`.
- **One sudo, documented:** `apt-get install --no-install-recommends pulseaudio
  pulseaudio-utils` (§2). The work order's "no sudo" line is read against the dispatch
  brief's explicit allowance for a dummy audio server on a VPS with no sound card; the
  install is the whole deviation, and `pulseaudio -k` restored the no-daemon state.
- `cargo fmt --all --check` → exit 0.
- `cargo test -p suramak-asr --lib --offline` → **150 passed, 0 failed, 1 ignored**
  (19.32 s), including `mix::tests::a_slow_source_stays_aligned_over_a_long_run`.
- Linux captures: real reads on this VPS, commands in §2, raw outputs
  `/tmp/m16g/parec_monitor_30s.raw` (914 482 B) and the probe's stdout lines quoted above.
- Windows: source-inspected at the cited lines of
  `~/.cargo/registry/src/index.crates.io-*/cpal-0.15.3`; no runtime claim is made.
- Disk: probe artifacts 21 MB in `/tmp/m16g-target`; `/` has 8.1 GB free (the workspace
  `target/` is 11 GB and was deliberately not cleaned — other work is using it).
