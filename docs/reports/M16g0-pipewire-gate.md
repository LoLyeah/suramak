# M16.5-S0L — the PipeWire desktop gate, closed headlessly

**Verdict: slice-0 Gate PASSED.** A real PipeWire 1.6.2 desktop stack (pipewire +
pipewire-pulse + WirePlumber) was installed on this VPS, a null sink carried a 440 Hz tone,
and the M16G probe re-ran unchanged in shape: the ALSA `pulse` PCM plus
`PULSE_SOURCE=@DEFAULT_MONITOR@` captured **29.986 s of audio in 30.000 s (RTF 0.9995)** with
the tone intact (RMS 0.17669 against the source's 0.17676), and a single process held the
monitor stream and a microphone-shaped stream at once. The production capture slice list
stands; the PipeWire findings fold into it as implementation details, not as a re-plan
(§7). What a real desktop still adds is the DAC clock and hardware device names — those are
slice 1/2 acceptance items, not a gate deferral (§8).

The single most important finding: on PipeWire, the ALSA **`default`** PCM is the native
`pipewire` plugin, **not** a libpulse client. It silently ignores `PULSE_SOURCE` and records
the default *source* (the microphone). Production must open the PCM named **`pulse`**
explicitly; the M16G report's "`default` as fallback" is dead on PipeWire (§5.3).

---

## 1. Machine-named numbers

```text
probe_date=2026-11-11
probe_host=VM-0-5-ubuntu
probe_cpu=AMD EPYC 7K62, 2 vCPU
probe_kernel=7.0.0-14-generic
os=Ubuntu 26.04 LTS
pipewire_version=1.6.2-1ubuntu1.2
pipewire_pulse_version=1.6.2-1ubuntu1.2
wireplumber_version=0.5.13-1ubuntu1.1
pipewire_alsa_version=1.6.2-1ubuntu1.2
pipewire_pulse_server_string=PulseAudio (on PipeWire 1.6.2)
pipewire_pulse_protocol=35
pipewire_default_rate_hz=48000
pipewire_default_sample_spec=float32le 2ch 48000Hz
cpal_version=0.15.3
libasound2_plugins_version=1.2.12-2build1
pulseaudio_baseline_version=17.0 (1:17.0+dfsg1-2ubuntu4, M16G run)

pipewire_host_count=1
pipewire_host_ids=Alsa
pipewire_input_device_count=3
pipewire_input_device_names=pipewire,pulse,default
pipewire_input_config_each=2ch@44100Hz/F32
pipewire_pulse_pcm_rate_range_hz=1..768000
pipewire_pulse_pcm_channels=1..32
pipewire_pulse_pcm_formats=U8,I16,I32,F32
pipewire_monitor_name=probe.monitor
pipewire_monitor_addressable_by_name=false
pipewire_null_sink_factory=support.null-audio-sink

pipewire_cpal_monitor_wall_s_30s=30.000
pipewire_cpal_monitor_frames_30s=1322400
pipewire_cpal_monitor_rate_hz_30s=44100
pipewire_cpal_monitor_channels_30s=2
pipewire_cpal_monitor_format_30s=F32
pipewire_cpal_monitor_audio_s_30s=29.986
pipewire_cpal_monitor_rtf_30s=0.9995
pipewire_cpal_monitor_rms_30s=0.17669
pipewire_cpal_monitor_peak_30s=0.24999
pipewire_cpal_monitor_zcr_hz_30s=441.7
pipewire_cpal_monitor_stream_errors_30s=0
pipewire_cpal_monitor_rtf_30s_runs=0.9995,0.9995,0.9995

pipewire_parec_monitor_bytes_30s=958720
pipewire_parec_monitor_audio_s_30s=29.960
pipewire_parec_monitor_rtf_30s=0.9955
pipewire_parec_monitor_rms_30s=0.17670
pipewire_parec_monitor_peak_30s=0.25180
pipewire_parec_monitor_zcr_hz_30s=440.4
pipewire_tone_freq_hz_autocorr=439.996
pipewire_tone_freq_autocorr_corr=1.000000
tone_source_freq_hz=440.000
tone_source_rms=0.17676

pipewire_cpal_rtf_10s_tone=0.9995
pipewire_cpal_rtf_10s_silent_a=0.9995
pipewire_cpal_rtf_10s_silent_b=0.9995
pipewire_cpal_rms_10s_silent_a=0.00000
pipewire_cpal_rms_10s_silent_b=0.00000

pipewire_dual_monitor_frames_8s=354844
pipewire_dual_monitor_audio_s_8s=8.046
pipewire_dual_monitor_rtf_8s=1.0058
pipewire_dual_monitor_rms_8s=0.17652
pipewire_dual_default_frames_8s=352640
pipewire_dual_default_audio_s_8s=7.996
pipewire_dual_default_rtf_8s=0.9995
pipewire_dual_default_rms_8s=0.00000
pipewire_dual_monitor_frames_15s=663404
pipewire_dual_monitor_audio_s_15s=15.043
pipewire_dual_monitor_rtf_15s=1.0029
pipewire_dual_default_frames_15s=661200
pipewire_dual_default_audio_s_15s=14.993
pipewire_dual_default_rtf_15s=0.9995

pipewire_default_monitor_resolution_sink_probe=source_output->55 (probe.monitor)
pipewire_default_monitor_resolution_sink_micnull=source_output->62 (micnull.monitor)
pipewire_default_pcm_capture_source=714 (mic, silent) rms=0.00000
pipewire_pulse_pcm_capture_source=698 (probe.monitor) rms=0.17629
pipewire_pulse_pcm_absent_when_server_stopped=true
```

## 2. What was installed, and every command

The VPS has no sound card (`/proc/asound/cards` absent; `/dev/snd` holds only `seq`/`timer`),
so "desktop" here means the full userspace stack with a null sink — the same substitution
M16G made for PulseAudio. A systemd user session was already running with
`XDG_RUNTIME_DIR=/run/user/1000` and `DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus`.

**The one sudo (install):**

```bash
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
     pipewire pipewire-pulse pipewire-alsa wireplumber
```

Installed (apt history, `2026-11-11 18:19:06`): `pipewire 1.6.2-1ubuntu1.2`,
`pipewire-bin 1.6.2-1ubuntu1.2`, `pipewire-pulse 1.6.2-1ubuntu1.2`,
`pipewire-alsa 1.6.2-1ubuntu1.2`, `wireplumber 0.5.13-1ubuntu1.1`,
`libpipewire-0.3-modules 1.6.2-1ubuntu1.2`, `libwireplumber-0.5-0 0.5.13-1ubuntu1.1`,
`liblua5.4-0`, `libroc0.4` (automatic). `libspa-0.2-modules 1.6.2-1ubuntu1.2` was already
present. `pulseaudio-utils` (which owns `pactl`, `parec`, `paplay`; `parec`/`paplay` are
symlinks to `pacat`) remained installed and works as a libpulse client against
pipewire-pulse — that is how the native-client column was measured.

**Side effect, documented:** `pipewire-alsa` declares `Conflicts: pulseaudio`, so apt
**removed the PulseAudio 17 binary** (`Remove: pulseaudio:amd64 (1:17.0+dfsg1-2ubuntu4)`;
`dpkg -s pulseaudio` → `deinstall ok config-files`). The M16G baseline numbers were taken
before this install and are quoted from its report; the M16G hygiene ("stop the daemon")
is satisfied a fortiori — `pgrep -a pulseaudio` is empty and there is no daemon binary
left to start (§9).

**Start the stack (socket activation, user session):**

```bash
systemctl --user start pipewire.socket pipewire-pulse.socket wireplumber.service
pactl info
# Server Name: PulseAudio (on PipeWire 1.6.2)
# Server Version: 15.0.0
# Default Sample Specification: float32le 2ch 48000Hz
```

**The probe graph (idempotent script, `/tmp/m16g0/setup.sh`):**

```bash
pactl load-module module-null-sink sink_name=probe   sink_properties=device.description=ProbeSink
pactl load-module module-null-sink sink_name=micnull sink_properties=device.description=MicNull
pactl load-module module-remap-source master=micnull.monitor source_name=mic \
      source_properties=device.description=MockMic        # a microphone-shaped, non-monitor source
pactl set-default-sink probe
pactl set-default-source mic
```

`probe.monitor` is the system-audio source; `mic` is the "microphone" the dual test uses so
the second stream is not another monitor. `auto_null` (PipeWire's own fallback sink) exists
until a sink is created; the probe graph supersedes it as the default.

**The tone** (the M16G file, verified: 45 s, 440 Hz, amplitude 0.25, 44.1 kHz stereo s16,
measured RMS 0.17676 / peak 0.24997). Reproducible equivalent:

```bash
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=45" \
       -af "volume=2.828" -ac 2 -c:a pcm_s16le /tmp/m16g/tone440.wav   # RMS 0.17671, peak 0.24991
paplay -d probe /tmp/m16g/tone440.wav
```

**The probe** (M16G harness, `cpal = 0.15.3`, git-ignored under `build/m16g-probe/`):

```bash
cd build/m16g-probe
CARGO_TARGET_DIR=/tmp/m16g-target cargo build --release --offline
/tmp/m16g-target/release/m16g-probe list                 # host + device inventory
/tmp/m16g-target/release/m16g-probe configs pulse        # advertised rates/formats
PULSE_SOURCE='@DEFAULT_MONITOR@' /tmp/m16g-target/release/m16g-probe open pulse 30
```

Two modes were added to the throwaway harness for this gate (still git-ignored, no
workspace change): `dualpcm <pcm> <monitor> <secs>` (the M16G `dual` shape but opening a
*named* PCM for both streams, because the host default is now the native pipewire PCM) and
`configs <pcm>`.

**The measurements** (scripts in `/tmp/m16g0/`, raw outputs in `/tmp/m16g0/out/`):

```bash
bash /tmp/m16g0/run_window.sh      # 30 s: paplay tone + parec(@DEFAULT_MONITOR@) + cpal open pulse 30
bash /tmp/m16g0/rtf_stability.sh   # 10 s tone + two 10 s silent runs
bash /tmp/m16g0/dual_test.sh       # 8 s dual: monitor + mic in one process
bash /tmp/m16g0/dual15.sh          # 15 s dual + source-output introspection
bash /tmp/m16g0/name_tests.sh      # token/PCM resolution matrix
bash /tmp/m16g0/resolve_test.sh    # which source @DEFAULT_MONITOR@ connects to
bash /tmp/m16g0/pcm_route_test.sh  # which source the `default` vs `pulse` PCM captures
bash /tmp/m16g0/midswitch_test.sh  # default-sink change while a capture is open
bash /tmp/m16g0/move_test2.sh      # default-sink change with only playback open
systemctl --user stop  pipewire-pulse.socket pipewire-pulse.service   # server-absent probe
systemctl --user start pipewire-pulse.socket
```

## 3. The probe re-run, side by side

30 s wall window, tone playing into the default sink, two independent libpulse clients
(`parec` native, cpal through the ALSA `pulse` plugin). The PipeWire repeat is exact: all
three 30 s runs produced **the same frame count and RTF** (1 322 400, 0.9995); RMS varied
only in the fifth decimal (0.17669–0.17673).

| metric (30 s window) | PulseAudio 17 (M16G) | PipeWire 1.6.2 | delta |
|---|---|---|---|
| cpal frames @44.1 kHz | 1 258 524 | 1 322 400 | +63 876 (+5.1 %) |
| cpal audio captured | 28.538 s | 29.986 s | +1.448 s |
| **cpal RTF** | **0.9513** | **0.9995** | +4.8 pp |
| cpal RMS / peak | 0.17676 / 0.24997 | 0.17669 / 0.24999 | −0.04 % / +0.01 % |
| cpal ZCR (crude estimator) | 440.0 Hz | 441.7 Hz | estimator noise (§3.1) |
| cpal stream errors | 0 | 0 | — |
| parec bytes @16 kHz mono | 914 482 | 958 720 | +4.8 % |
| parec audio / RTF | 28.578 s / 0.9520 | 29.960 s / 0.9955 | +4.8 pp |
| parec RMS | 0.17676 | 0.17670 | −0.03 % |
| 10 s silent run A / B | 0.9437 / 0.9837 | 0.9995 / 0.9995 | — |

**The 0.9513 was the PulseAudio null sink's clock, and PipeWire's null sink does not have
that defect.** Both independent clients gained the same ~4.8 % on PipeWire, so this is the
server/sink clock, not cpal. PipeWire's sink is the `support.null-audio-sink` SPA node
(measured via `pw-dump`), a timer-clocked virtual sink; the audio-time deficit is 0.05 %,
i.e. inside the gate's [0.99, 1.01].

The M16G caveat "virtual sinks blow past the mixer's ±2 % drift clamp (−48 700 ppm)" is
therefore a **PulseAudio-null-sink** artifact, not a Linux null-sink artifact: on PipeWire
`MixOutcome.reports[].clamped` would be false (ρ ≈ 0.9995). The rule "do not validate
drift on a null sink" still stands — a null sink is not a DAC.

**Silence keeps ticking** on PipeWire too: two 10 s monitor reads with nothing playing
delivered 9.994 s each (RTF 0.9995, RMS 0.00000). No gap-fill is needed on Linux, matching
M16G's conclusion.

### 3.1 Tone intactness

RMS and peak match the source to within 0.04 %, so the monitor path carries the actual sink
audio at the same level. The zero-crossing estimator that M16G used reports 440.4–441.7 Hz
here because PipeWire resamples 48 kHz→44.1 kHz (and then parec 44.1→16 kHz) and the
crossing interpolation is sensitive to the resampler's ringing; a 5 s parabolic-interpolated
autocorrelation on the parec capture gives **439.996 Hz (correlation 1.000000)**. The tone is
440 Hz, not shifted; the ZCR column is a crude estimator, not a pitch measurement.

## 4. Dual-stream coexistence (the M6 mixer requirement)

One process, two cpal input streams, both on the ALSA `pulse` PCM, `PULSE_SOURCE` set only
around the first `build_input_stream` and removed before the second. Stream A = default
monitor (tone), stream B = the process default source `mic` (a remap of a silent null sink,
i.e. microphone-shaped and not a monitor). 8 s window, the same window M16G used:

```text
pcm="pulse" stream_monitor=PULSE_SOURCE:@DEFAULT_MONITOR@ stream_default=env-cleared
stream=monitor rate=44100 channels=2 frames=354844 audio_s=8.046 rtf=1.0058 rms=0.17652 peak=0.24998 zcr_hz=440.7 errors=[]
stream=default rate=44100 channels=2 frames=352640 audio_s=7.996 rtf=0.9995 rms=0.00000 peak=0.00000 zcr_hz=0.0 errors=[]
```

`pactl list short source-outputs` during the 15 s repeat shows the two connections
explicitly: stream 749 → source 698 (`probe.monitor`) and stream 764 → source 714 (`mic`).
Both streams delivered full audio-time, no errors, and the monitor stream is the tone while
the mic stream is silent — the per-open `PULSE_SOURCE` semantics survive on pipewire-pulse.

Baseline comparison (M16G, 8 s): monitor 289 897 frames (6.574 s), default 345 714 frames
(7.839 s) — PipeWire delivered 8.046 s and 7.996 s in the same shape. The M16G baseline's
monitor stream lost 18 % of the window under dual load on the PulseAudio null sink; PipeWire
lost none.

## 5. Device-name strategy on PipeWire

### 5.1 What cpal lists

`cpal 0.15.3` still has exactly one Linux host, ALSA. With pipewire-alsa installed the
inventory grows from M16G's two rows to three:

```text
host=ALSA input_devices=["pipewire [2ch@44100Hz/F32]", "pulse [2ch@44100Hz/F32]", "default [2ch@44100Hz/F32]"]
host=ALSA output_devices=["pipewire", "pulse", "default"]
```

The `pulse` PCM still advertises 1–32 channels and 1–768 000 Hz (U8/I16/I32/F32), default
2 ch @ 44.1 kHz. `probe.monitor` and `@DEFAULT_MONITOR@` are still **not** addressable as
ALSA devices (`try probe.monitor` → `found=false error=no-such-input-device`).

### 5.2 What the Settings picker would show

`list_input_devices_detailed` (`capture.rs:498-513`) returns the host's input devices
verbatim, so the picker shows **three** rows — `pipewire`, `pulse`, `default` — with
`default` marked as the default (cpal's `default_input_device()` is the ALSA `default`).
All three are the same audio server; none is a microphone in the physical sense. This is a
UX regression against M16G's two rows and slice 3 must dedupe: one "System default
microphone" row (open `pulse`; plain capture through `default` also lands on the default
source) plus the existing "System audio" source type. Monitors must still never appear as
devices.

On a machine without PipeWire (M16G baseline) the same call returns `["default", "pulse"]`;
with pipewire-pulse stopped the `pulse` row disappears (`found=false`) while `pipewire` and
`default` remain (§5.5). The picker therefore changes with server state — expected, and a
usable "system audio unavailable" signal.

### 5.3 Which PCM can see `PULSE_SOURCE` — and which silently cannot

Measured with the tone playing and the default source deliberately set to the silent `mic`:

| `PULSE_SOURCE` | PCM opened | RMS | ZCR | source-output target |
|---|---|---|---|---|
| `@DEFAULT_MONITOR@` | `pulse` | 0.17629 | 440.8 | 698 `probe.monitor` |
| `@DEFAULT_MONITOR@` | `default` | 0.00000 | 0.0 | **714 `mic`** |
| `@DEFAULT_MONITOR@` | `pipewire` | 0.00000 | 0.0 | default source (not introspected) |

`default` and `pipewire` are the native PipeWire ALSA plugin: not libpulse clients, no
`PULSE_SOURCE` handling, and they capture the *default source*. Asking for system audio
through `default` on a PipeWire desktop yields a perfectly healthy silent stream from the
microphone — the worst failure mode. Production must open **`pulse` by name** and treat its
absence as "system audio unavailable".

### 5.4 Does `@DEFAULT_MONITOR@` resolve, and does it track the default sink?

Yes, on both counts, through pipewire-pulse 1.6.2:

- `PULSE_SOURCE=@DEFAULT_MONITOR@` captured the tone for 30 s (§3).
- `PULSE_SOURCE=probe.monitor` (concrete name) and `PULSE_SOURCE=@DEFAULT_SINK@.monitor`
  also captured the tone (RMS 0.17580 / 0.17581 in the 2 s matrix).
- With the default sink `probe`, the capture's source-output pointed at 55 (`probe.monitor`);
  after `pactl set-default-sink micnull` plus a 0.5 s settle, the next open pointed at
  62 (`micnull.monitor`). The token is resolved **per open**, at connect time.
- `PULSE_SOURCE=@DEFAULT_SOURCE@` captured the default source `mic` (RMS 0) — it is not a
  monitor alias, as expected.

Do not infer resolution from RMS while a tone is playing: changing the default sink can move
the playing stream with it (§5.5), so both old and new monitor may carry the tone. The
source-output list is the only clean observation.

### 5.5 Failure modes, measured

- **Unresolvable `PULSE_SOURCE` does not fail the open.** `PULSE_SOURCE=no.such.source`
  produced a normal 2 s stream from the default source (RMS 0.00000, no error). The
  client-side stack falls back to the default source when the requested source is missing.
  Since production only ever passes the fixed `@DEFAULT_MONITOR@` token, this is unreachable
  in the shipped path — but if the token were ever unsupported, the app would silently
  record the microphone. Slice 1 must not expose a free-text source name on Linux, and its
  acceptance test is a tone test (§7).
- **Server absent.** With `pipewire-pulse.service` stopped, the `pulse` PCM disappears from
  cpal's enumeration entirely (`found=false error=no-such-input-device`); `pipewire` and
  `default` remain. Nothing crashes; slice 3's "disabled with the reason" is implementable
  by name lookup.
- **Restarting pipewire-pulse destroys module-created sinks.** The null sinks live in the
  pulse server process; a service restart left only `auto_null`. This is a probe-environment
  fact, not a production one (hardware sinks are owned by WirePlumber/PipeWire), but any
  scripted test that restarts the server must recreate its graph.
- **Changing the default sink can move live playback.** With only playback open,
  `pactl set-default-sink micnull` moved the playing `paplay` stream from sink 698 to 706
  (two runs, `Sink:` field); with a monitor capture open on `probe`, two runs showed the
  playback staying on `probe` (RMS unchanged across the switch). The move policy is
  conditional and not client-visible. Consequence for tests: never switch the default sink
  while a tone is playing and infer the token's resolution from RMS — read
  `pactl list short source-outputs` instead (that is how §5.4 was measured). Consequence for
  production: a monitor stream is pinned at open; a mid-recording output switch is a
  slice-2 acceptance item (§8).

### 5.6 Strategy, stated exactly

- **Host** = cpal ALSA (the only Linux host). **Device** = the ALSA PCM named **`pulse`**
  (libasound2-plugins → libpulse → pipewire-pulse). Not `default`, not `pipewire`.
- **Source** = `PULSE_SOURCE=@DEFAULT_MONITOR@`, scoped to that one open (set → open →
  restore under the process-wide audio-open mutex + drop guard).
- **Availability** = the `pulse` PCM appears in `host.input_devices()`. Its absence is the
  only server-absent signal needed; no `pactl`, no server introspection, no new dependency.
- **Picker** = one microphone row + the existing "System audio" source type; the three ALSA
  pseudo-devices are not user-facing choices. The picker never needs the PipeWire node name:
  the token is resolved by the server at connect time.

## 6. The gate criteria, one by one

| Plan's slice-0 criterion (M16G §6) | Measured on PipeWire 1.6.2 | Result |
|---|---|---|
| The ALSA `pulse` PCM exists | yes — `pulse`, plus native `pipewire` and `default` | PASS |
| `@DEFAULT_MONITOR@` resolves through pipewire-pulse | yes — 30 s tone, 439.996 Hz autocorrelation | PASS |
| 30 s monitor read RTF in [0.99, 1.01] | 0.9995 (cpal), 0.9955 (parec), three repeats | PASS |
| Tone intact | RMS 0.17669 vs source 0.17676, peak 0.24999 vs 0.24997 | PASS |
| Dual-stream coexistence (M6) | monitor 8.046 s / mic 7.996 s in one process, both full-rate | PASS |
| No new dependency | probe still `cpal = 0.15.3` only; workspace untouched | PASS |

## 7. The production slice list, with the PipeWire findings folded in

Slices 1–4 keep their gates; the changes are implementation details that the measurements
now pin down.

| # | Slice | Folded-in PipeWire finding |
|---|---|---|
| 0 | **PipeWire desktop probe** | **Closed here (PASSED).** No longer a hardware-only gate. |
| 1 | Linux SystemAudio open | Open the ALSA **`pulse`** PCM **by name**; if `pulse` is absent, report system audio unavailable (do not fall back to `default` — it records the microphone silently). Keep `PULSE_SOURCE=@DEFAULT_MONITOR@` scoped per open under the process-wide audio-open mutex + drop guard; the dual test proves per-open semantics hold on pipewire-pulse. Never expose a free-text source name. |
| 2 | Recorder wiring | `Recorder.device_name` = "System audio (monitor)", rate from the plugin config (default 44.1 kHz; the PCM accepts 1–768 kHz, so 48 kHz native is available if desired). Add the mid-session output-switch case: the monitor is pinned at open; an output switch needs a re-open or an explicit warning. |
| 3 | Settings/UI | Dedupe the three ALSA pseudo-devices (`pipewire`, `pulse`, `default`) into one "System default microphone" row + the "System audio" source type; hide pseudo-devices from the mic list. System audio row disabled when `pulse` is not enumerated (measured: it vanishes with the server). |
| 4 | Telemetry | Log per-source RTF and `clamped`. On PipeWire's null sink RTF ≈ 1.0 (clamp off); a real sink should be ≈ 1.0. Keep the `RTF < 0.9 = source problem` warning. |

**Why not DEFER:** the gate existed to answer whether pipewire-pulse resolves
`@DEFAULT_MONITOR@` and whether the ALSA `pulse` PCM remains a libpulse client on a
PipeWire desktop. Both are answered positively on a real PipeWire 1.6.2 + WirePlumber
stack. Headless changes the sink *clock* (timer-driven null sink instead of a DAC) and the
device *names* (no hardware), not the capture mechanism. Those are precisely the things
slice 1/2 acceptance on the owner's desktop must observe, and they cannot invalidate the
mechanism the way a missing `pulse` PCM or an unresolved token would have.

**Re-visit condition (unchanged in spirit):** if a real desktop lacks the ALSA `pulse` PCM
(no libasound2-plugins), system audio is unavailable there; if its PipeWire version does not
resolve `@DEFAULT_MONITOR@`, use the concrete `<default-sink>.monitor` from
`pactl get-default-sink` (verified working here), and only then consider the
libpulse-simple dependency.

## 8. What a real desktop still adds (and why it does not block)

- **A DAC-clocked hardware sink.** RTF should be ~1.0 with ~±100 ppm; the null sink's
  0.9995 is a timer, not a crystal. Slice 1's acceptance already requires a 30 s tone read
  with RTF in [0.99, 1.01] — that test is unchanged and now has a known-good software
  reference.
- **Real device names and a real default sink.** The token resolves to the hardware sink's
  monitor; the name is `<sink>.monitor` only when introspection is used. The design never
  needs that name.
- **Hot-plug and mid-session output switching.** Measured here only as "the monitor is
  pinned at open" (§5.5); slice 2 must decide re-open vs warn.
- **48 kHz-native capture.** This box's `pulse` PCM defaults to 44.1 kHz and PipeWire
  resamples its 48 kHz graph; a desktop with 48 kHz hardware would avoid one resample. The
  mixer handles both (`TARGET_RATE = 16_000`), so this is a quality nicety, not a gate.
- **WirePlumber policy with real hardware** (suspend-on-idle, echo-cancel sources, etc.) —
  unmeasured; the "silence keeps ticking" result is from a null sink.

## 9. Verification of this work order

- **No production code, no dependency, no CI change.** `git status --porcelain` shows exactly
  one entry — `?? docs/reports/M16g0-pipewire-gate.md`. Nothing under `crates/`, `Cargo.toml`,
  or `.github/` was touched by this work (the Android/CI changes visible in the tree are
  another task's, already committed as `ed97cc7`). The probe harness changes (`dualpcm`,
  `configs`) live in the git-ignored `build/m16g-probe/` (`git check-ignore` →
  `.gitignore:13:build/`).
- `cargo fmt --all --check` → exit 0.
- `cargo clippy --workspace --all-targets --offline -- -D warnings` → exit 0.
- `cargo test -p suramak-asr --lib --offline` → **163 passed, 0 failed, 1 ignored** (19.11 s).
- `cargo test --workspace --offline` → **1130 passed, 0 failed** across 36 test binaries
  (second full run). The first full run hit one pre-existing flaky test in
  `suramak-android-spike` (`diag::tests::the_panic_hook_records_without_deadlocking`, a
  global crash-state/ordering race); it passes in isolation and in the re-run, and this
  work touched no Rust source.
- **PulseAudio daemon stopped.** `pgrep -a pulseaudio` → no process. The `pulseaudio`
  package was removed by `pipewire-alsa`'s `Conflicts` during the documented install
  (`dpkg -s pulseaudio` → `deinstall ok config-files`), so there is no binary left to run;
  the M16G baseline numbers are quoted from its report, not re-measured.
- **Disk.** `cargo clean` removed 13.9 GiB of `target/` after the tests; `/` is back to
  18 GB free (69 %). Probe artifacts: 21 MB in `/tmp/m16g-target`, 1.2 MB in `/tmp/m16g0`
  (both tmpfs).
- **Installed packages** (final state): `pipewire`, `pipewire-bin`, `pipewire-pulse`,
  `pipewire-alsa` 1.6.2-1ubuntu1.2; `wireplumber` 0.5.13-1ubuntu1.1; `libpipewire-0.3-*`,
  `libwireplumber-0.5-0`; `libasound2-plugins` 1.2.12-2build1 (pre-existing).
  `pipewire.service`, `pipewire-pulse.service` (socket-activated) and `wireplumber.service`
  are active in the `ubuntu` user session and were left running.
