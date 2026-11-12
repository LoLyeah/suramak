#!/usr/bin/env bash
# Suramak — M13I: the Android launch smoke on an x86_64 emulator.
#
#   scripts/android/emulator-smoke.sh
#
# Proves, on a real Android runtime, what no host test can: the debug APK
# installs, its process starts, the compact Library surface paints, the app
# survives a bounded monkey pass, and the system logged no fatal error. A
# launch failure is a failed exit code — this script is the job's gate, not a
# report generator.
#
# M19A adds the phone-interaction steps: a tap opens Settings, a swipe moves the
# list (proved by a screencap diff with a still-frame negative control), a tap
# after the swipe still fires (proved by the shell's route marker), and the
# **Quit** row — the list's real last control — ends above the gesture inset at
# full scroll (proved by the shell's SURAMAK-LAYOUT probe, on its resting line).
# M19A2 crops the status-bar band out of both diff frames (the clock is not the
# app's) and reads the probe's settled line by its own timestamp. The same run
# records Step 0's two answers in `input-log.txt`: whether pointer events
# already reach egui, and whether the touch coordinates are egui points.
#
# Requires (all provided by the CI job):
#   - an x86_64 AVD package installed (`system-images;android-35;google_apis;x86_64`)
#   - a working /dev/kvm (the emulator refuses to boot usefully without it)
#   - platform-tools (`adb`), `emulator`, `avdmanager` in $ANDROID_HOME
#   - an x86_64 APK built by `SURAMAK_ANDROID_ABI=x86_64 scripts/android/build-apk.sh`
#
# Environment (all optional):
#   SURAMAK_APK             APK to install (default android/app/build/outputs/apk/debug/app-debug.apk)
#   SURAMAK_AVD_NAME        AVD name (default suramak-m13i)
#   SURAMAK_SYSTEM_IMAGE    (default system-images;android-35;google_apis;x86_64)
#   SURAMAK_AVD_DEVICE      avdmanager device profile (default pixel_6)
#   SURAMAK_OUT             artifact directory (default build/android-smoke)
#   SURAMAK_BOOT_TIMEOUT    seconds to wait for boot (default 600)
#   SURAMAK_LAUNCH_TIMEOUT  seconds to wait for the first frame marker (default 90)
#   SURAMAK_FINISH_TIMEOUT  seconds to wait for the process to leave after BACK
#                           (default 20; the M13I-b finish-path exit)
#   SURAMAK_MONKEY_EVENTS   monkey event count (default 30; 30 × 250 ms ≈ 7.5 s
#                           of injection, ~10 s wall including monkey's startup)
#   SURAMAK_KEEP_RUNNING=1  leave the emulator up (local debugging only)
#
# Exit codes: 0 = the smoke passed; 1 = an assertion failed; 2 = the harness
# could not run (no KVM, no AVD, no APK).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

APK="${SURAMAK_APK:-android/app/build/outputs/apk/debug/app-debug.apk}"
AVD_NAME="${SURAMAK_AVD_NAME:-suramak-m13i}"
SYSTEM_IMAGE="${SURAMAK_SYSTEM_IMAGE:-system-images;android-35;google_apis;x86_64}"
AVD_DEVICE="${SURAMAK_AVD_DEVICE:-pixel_6}"
OUT="${SURAMAK_OUT:-build/android-smoke}"
BOOT_TIMEOUT="${SURAMAK_BOOT_TIMEOUT:-600}"
LAUNCH_TIMEOUT="${SURAMAK_LAUNCH_TIMEOUT:-90}"
FINISH_TIMEOUT="${SURAMAK_FINISH_TIMEOUT:-20}"
LIBRARY_TIMEOUT="${SURAMAK_LIBRARY_TIMEOUT:-300}"
MONKEY_EVENTS="${SURAMAK_MONKEY_EVENTS:-30}"
PACKAGE="app.suramak.spike"
ACTIVITY="$PACKAGE/.MainActivity"

fail() {
  echo "SMOKE-FAIL: $*" >&2
  # Always leave the app's own logcat in the artifact dir: a failure is the
  # one time its lines matter most (the Library-ready wait failed once with
  # no app log uploaded — the verdict had to be re-derived from a rerun).
  if [ -n "${OUT:-}" ] && [ -d "${OUT:-}" ] && command -v "$ADB" >/dev/null 2>&1; then
    timeout 30 "$ADB" logcat -d >"$OUT/logcat-on-fail.txt" 2>/dev/null || true
    timeout 30 "$ADB" logcat -d -s SuramakSpike:I >"$OUT/logcat-app-on-fail.txt" 2>/dev/null || true
    echo "   logs:     $OUT/logcat-on-fail.txt, $OUT/logcat-app-on-fail.txt" >&2
  fi
  exit 1
}

harness_error() {
  echo "SMOKE-HARNESS-ERROR: $*" >&2
  exit 2
}

# --- toolchain discovery ------------------------------------------------------
SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/android-sdk}}"
[ -d "$SDK" ] || harness_error "no Android SDK at $SDK (set ANDROID_HOME)"
ADB="$SDK/platform-tools/adb"
EMULATOR="$SDK/emulator/emulator"
AVDMANAGER="$SDK/cmdline-tools/latest/bin/avdmanager"
if [ ! -x "$AVDMANAGER" ]; then
  AVDMANAGER="$(command -v avdmanager || true)"
fi
for tool in "$ADB" "$EMULATOR"; do
  [ -x "$tool" ] || harness_error "missing $tool (install the emulator + platform-tools packages)"
done
[ -n "$AVDMANAGER" ] && [ -x "$AVDMANAGER" ] || harness_error "avdmanager not found"
[ -f "$APK" ] || harness_error "no APK at $APK (build it with SURAMAK_ANDROID_ABI=x86_64 scripts/android/build-apk.sh)"
[ -e /dev/kvm ] || harness_error "/dev/kvm is absent — the emulator cannot run without KVM"

export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"
export PATH="$SDK/platform-tools:$SDK/emulator:$PATH"
mkdir -p "$OUT"
# avdmanager refuses an ANDROID_AVD_HOME that does not exist yet (the CI job
# pins it inside the workspace so a stale runner-home AVD cannot leak in).
if [ -n "${ANDROID_AVD_HOME:-}" ]; then
  mkdir -p "$ANDROID_AVD_HOME"
fi

echo "== Suramak emulator smoke (M13I) =="
echo "   apk:      $APK ($(stat -c %s "$APK") bytes)"
echo "   image:    $SYSTEM_IMAGE"
echo "   avd:      $AVD_NAME ($AVD_DEVICE)"
echo "   out:      $OUT"

# --- AVD ----------------------------------------------------------------------
# Capture the listing first: `avdmanager list avd | grep -q` under `pipefail`
# can report the writer's SIGPIPE as "not found" (the repo's own lesson).
AVD_LIST="$("$AVDMANAGER" list avd 2>/dev/null || true)"
case "$AVD_LIST" in
  *"Name: $AVD_NAME"*) echo "   reusing AVD $AVD_NAME" ;;
  *)
    echo "== creating AVD $AVD_NAME =="
    # `< /dev/null` answers the custom-hardware-profile prompt with its default
    # ("no") without a live `echo |` pipeline under pipefail.
    "$AVDMANAGER" create avd \
      --name "$AVD_NAME" \
      --package "$SYSTEM_IMAGE" \
      --device "$AVD_DEVICE" \
      --force </dev/null
    ;;
esac

EMULATOR_PID=""
cleanup() {
  local status=$?
  if [ -n "$EMULATOR_PID" ] && [ "${SURAMAK_KEEP_RUNNING:-0}" != "1" ]; then
    "$ADB" emu kill >/dev/null 2>&1 || true
    # The emulator process can outlive the console kill by a beat.
    for _ in $(seq 1 10); do
      kill -0 "$EMULATOR_PID" 2>/dev/null || break
      sleep 1
    done
    kill -9 "$EMULATOR_PID" 2>/dev/null || true
  fi
  exit "$status"
}
trap cleanup EXIT

# --- boot ---------------------------------------------------------------------
echo "== booting the emulator (headless, ANGLE) =="
# -no-window: no display on a runner; -gpu angle_indirect: the app's wgpu
# backend list is VULKAN|GL and swiftshader_indirect's GPU emulation
# SEGFAULTED mid-GPU-init while the app painted its first frame (observed in
# CI: the emulator process core-dumped during the first-frame wait) — ANGLE's
# translation layer is the supported GPU path on modern emulators.
# -wipe-data: a fresh first-run state every run (the onboarding sheet is part
# of the launch surface, so the smoke expects it);
# -no-snapshot/-no-boot-anim: no stale state, no animation delay.
# -memory 8192 (M19A): the model table is the device tier's, and the tier is
# read from usable RAM. At 2048 MB the phone reports "below the 4 GB floor" and
# offers ONE model row — the Settings list then fits the screen and there is
# nothing to scroll, which would make the swipe assertion vacuous. 8 GB is the
# tier the owner's phone and the M19 model guidance target.
GPU_MODE="${SURAMAK_EMU_GPU:-angle_indirect}"
"$EMULATOR" -avd "$AVD_NAME" \
  -no-window -no-audio -no-boot-anim -no-snapshot -wipe-data \
  -gpu "$GPU_MODE" -accel on \
  -memory 8192 -partition-size 4096 \
  -camera-back none -camera-front none \
  -no-metrics \
  >"$OUT/emulator.log" 2>&1 &
EMULATOR_PID=$!

# Wait for a device, then pin ANDROID_SERIAL: the port is not always 5554.
DEADLINE=$((SECONDS + BOOT_TIMEOUT))
SERIAL=""
while [ "$SECONDS" -lt "$DEADLINE" ]; do
  SERIAL="$("$ADB" devices | awk '/^emulator-[0-9]+[[:space:]]+device/ {print $1; exit}')"
  [ -n "$SERIAL" ] && break
  if ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
    tail -40 "$OUT/emulator.log" >&2 || true
    harness_error "the emulator process exited during boot"
  fi
  sleep 2
done
[ -n "$SERIAL" ] || harness_error "no emulator device appeared within ${BOOT_TIMEOUT}s"
export ANDROID_SERIAL="$SERIAL"
echo "   serial:   $SERIAL"

boot_completed() {
  [ "$("$ADB" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ] || return 1
  # Capture then match (SIGPIPE + pipefail: `adb | grep -q` reports a
  # non-match the moment grep exits early).
  local svc
  svc="$("$ADB" shell service check package 2>/dev/null || true)"
  case "$svc" in
    *found*) return 0 ;;
  esac
  return 1
}
while [ "$SECONDS" -lt "$DEADLINE" ]; do
  if boot_completed; then
    break
  fi
  sleep 3
done
boot_completed || {
  "$ADB" shell getprop | grep -E "boot_completed|bootanim" >&2 || true
  tail -60 "$OUT/emulator.log" >&2 || true
  harness_error "Android did not finish booting within ${BOOT_TIMEOUT}s"
}
echo "   boot:     complete"

# Deterministic input: no window animations, unlocked, animations off.
"$ADB" shell settings put global window_animation_scale 0
"$ADB" shell settings put global transition_animation_scale 0
"$ADB" shell settings put global animator_duration_scale 0
# M19A2: gesture navigation. The bottom-clearance assertion below is about the
# gesture pill, and an AVD can boot in three-button mode (a 48 dp bar, no
# pill). Enabling the gestural overlay before the app starts means the insets
# the shell sees are the ones the assertion names. Best-effort: on an image
# without the overlay the `settings` path is the fallback, and the assertion
# itself is the real gate.
"$ADB" shell cmd overlay enable-exclusive --category com.android.internal.systemui.navbar.gestural \
  >/dev/null 2>&1 \
  || "$ADB" shell settings put secure navigation_mode 2 >/dev/null 2>&1 \
  || true
"$ADB" shell input keyevent 82 >/dev/null 2>&1 || true
"$ADB" shell wm dismiss-keyguard >/dev/null 2>&1 || true
sleep 3

# The launcher shot is the baseline the app shot must differ from.
"$ADB" exec-out screencap -p >"$OUT/launcher.png"
"$ADB" shell wm size >"$OUT/wm-size.txt"
"$ADB" shell wm density >>"$OUT/wm-size.txt"

# --- install ------------------------------------------------------------------
echo "== installing $APK =="
"$ADB" install -r -t "$APK" >"$OUT/install.txt"
grep -q "Success" "$OUT/install.txt" || {
  cat "$OUT/install.txt" >&2
  fail "adb install did not report Success"
}
# Launch-time permissions: notifications (API 33+) and the microphone. Granting
# the microphone keeps the app on the Library if the monkey taps Record; the
# launch path itself never asks for it.
"$ADB" shell pm grant "$PACKAGE" android.permission.POST_NOTIFICATIONS >/dev/null 2>&1 || true
"$ADB" shell pm grant "$PACKAGE" android.permission.RECORD_AUDIO >/dev/null 2>&1 || true
"$ADB" shell dumpsys package "$PACKAGE" | grep -E "versionName|versionCode" >"$OUT/package.txt" || true

# --- launch -------------------------------------------------------------------
echo "== launching $ACTIVITY =="
"$ADB" logcat -c || true
"$ADB" shell am start -W -n "$ACTIVITY" >"$OUT/am-start.txt" 2>&1 || {
  cat "$OUT/am-start.txt" >&2
  fail "am start failed"
}
cat "$OUT/am-start.txt"

wait_for_log() {
  local pattern="$1" deadline=$((SECONDS + LAUNCH_TIMEOUT)) attempt=1
  while [ "$SECONDS" -lt "$deadline" ]; do
    # The emulator dying mid-wait must end the wait with a PRECISE verdict:
    # observed in CI (swiftshader): the emulator core-dumped during the
    # first-frame wait, and the smoke then blamed the app for a missing
    # marker. If the emulator process is gone, name it and dump its log.
    if ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      echo "== the EMULATOR PROCESS DIED mid-wait (pid $EMULATOR_PID) ==" >&2
      tail -40 "$OUT/emulator.log" >&2 2>/dev/null || true
      return 2
    fi
    # Each logcat -d is hard-bounded: on the swiftshader emulator the app's
    # GPU loop pegs the cores and an unbounded logcat can hang the whole
    # loop (observed: the step blew its 30-minute CI timeout inside one
    # logcat call). 20 s per attempt, marker grepped over the full log —
    # the marker text is unique, so no -s tag filter needed.
    # Capture first, match after: `logcat | grep -q` under `pipefail` reports
    # logcat's SIGPIPE (141) as a non-match the moment grep exits early — the
    # marker was IN the buffer and the wait still timed out (the input-log
    # flood made the race almost certain).
    local captured
    captured="$(timeout 20 "$ADB" logcat -d 2>/dev/null || true)"
    case "$captured" in
      *"$pattern"*) return 0 ;;
    esac
    echo "   wait:     ${pattern} (attempt ${attempt})"
    attempt=$((attempt + 1))
    sleep 2
  done
  return 1
}

# The compact host's first REAL Library draw can lag the first GPU frame by
# tens of seconds on a software-rendered emulator (the startup placeholder
# paints the whole time). The screencap gate inspects the LIBRARY, so wait
# for the shell's own ready marker before capturing — otherwise the gate
# inspects a near-black placeholder and mis-fails the launch (observed in CI).
wait_for_library_ready() {
  local deadline=$((SECONDS + LIBRARY_TIMEOUT))
  while [ "$SECONDS" -lt "$deadline" ]; do
    local captured
    captured="$(app_log)"
    case "$captured" in
      *"compact library surface: ready"*)
        echo "   library:  ready (compact host up, first real draw)"
        return 0
        ;;
    esac
    if ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      echo "== the EMULATOR PROCESS DIED mid-wait (pid $EMULATOR_PID) ==" >&2
      tail -40 "$OUT/emulator.log" >&2 2>/dev/null || true
      fail "the emulator died while waiting for the compact Library to come up"
    fi
    sleep 3
  done
  return 1
}



if ! wait_for_log "android_main entered"; then
  timeout 30 "$ADB" logcat -d >"$OUT/logcat-launch-failed.txt" 2>/dev/null || true
  tail -80 "$OUT/logcat-launch-failed.txt" >&2 || true
  fail "no 'android_main entered' marker in logcat — the native thread never started"
fi

# --- M13I-b: the relaunch contract -------------------------------------------
# The CI repro (run 43017092638): launch OK → BACK during the splash → the
# Activity is destroyed → the loop returns → the process stayed alive → the
# next launch's android_main could not create a second EventLoop → the JNI side
# drove the dead runtime → ApplicationExitInfo reason=2 (SIGNALED) status=11
# (SIGSEGV). The fix makes the end of the loop terminal (the process exits, so
# the next launch is a fresh process) and makes the loop-unavailable path exit
# cleanly too. These helpers assert exactly that, from logcat's threadtime
# columns: $3 is the pid that wrote the line.

# The app's own log, tag-filtered and time-bounded like `wait_for_log` (an
# unbounded logcat on a wedged emulator can hang the whole step).
app_log() {
  timeout 20 "$ADB" logcat -d -s SuramakSpike:I 2>/dev/null || true
}

# `<pid> <entry>` for the newest `android_main entered` line; empty if none.
latest_main_entry() {
  local line pid entry
  line="$(app_log | grep -F "android_main entered (entry #" | tail -1 || true)"
  [ -n "$line" ] || return 1
  pid="$(printf '%s\n' "$line" | awk '{print $3}')"
  entry="$(printf '%s\n' "$line" | sed -nE 's/.*android_main entered \(entry #([0-9]+).*/\1/p')"
  [ -n "$pid" ] && [ -n "$entry" ] || return 1
  printf '%s %s\n' "$pid" "$entry"
}

# The pid wrote <pattern> (fixed string)? ($1 pid, $2 pattern)
pid_log_has() {
  # Capture the pid's lines first, then match — see wait_for_log's SIGPIPE note.
  local lines
  lines="$(app_log | awk -v pid="$1" '$3 == pid' || true)"
  case "$lines" in
    *"$2"*) return 0 ;;
  esac
  return 1
}

# Does the package have no process at all?
package_pids() {
  "$ADB" shell pidof "$PACKAGE" 2>/dev/null | tr -d '\r' || true
}

# M13I-b layer 1: after BACK the finish path must end the process. A process
# that lingers is the state that produced the SIGSEGV on the next launch.
wait_for_process_exit() {
  local deadline=$((SECONDS + FINISH_TIMEOUT)) pids attempt=1
  while [ "$SECONDS" -lt "$deadline" ]; do
    pids="$(package_pids)"
    [ -z "$pids" ] && return 0
    if ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      echo "== the EMULATOR PROCESS DIED mid-wait (pid $EMULATOR_PID) ==" >&2
      tail -40 "$OUT/emulator.log" >&2 2>/dev/null || true
      return 2
    fi
    echo "   wait:     the app process to leave after BACK (pid $pids, attempt $attempt)" >&2
    attempt=$((attempt + 1))
    sleep 1
  done
  return 1
}

# The second launch must be a FRESH process. winit allows one EventLoop per
# process, so an in-process re-entry is the crash shape: a fresh process logs
# `entry #1` again (the counter is process-wide) under a NEW pid, while the old
# bug logged `entry #2` under the SAME pid. Prints "<pid> <entry>".
wait_for_fresh_launch() {
  local deadline=$((SECONDS + LAUNCH_TIMEOUT)) info attempt=1
  while [ "$SECONDS" -lt "$deadline" ]; do
    if ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      echo "== the EMULATOR PROCESS DIED mid-wait (pid $EMULATOR_PID) ==" >&2
      tail -40 "$OUT/emulator.log" >&2 2>/dev/null || true
      return 2
    fi
    info="$(latest_main_entry || true)"
    if [ -n "$info" ] && [ "${info%% *}" != "$FIRST_PID" ]; then
      printf '%s\n' "$info"
      return 0
    fi
    echo "   wait:     a fresh-process android_main (attempt $attempt)" >&2
    attempt=$((attempt + 1))
    sleep 2
  done
  return 1
}

# Wait for <pattern> in the lines written by <pid> only: the first-frame marker
# of the new generation must come from the new process, not from the old one.
wait_for_pid_log() {
  local pid="$1" pattern="$2" deadline=$((SECONDS + LAUNCH_TIMEOUT)) attempt=1
  while [ "$SECONDS" -lt "$deadline" ]; do
    if pid_log_has "$pid" "$pattern"; then
      return 0
    fi
    if ! kill -0 "$EMULATOR_PID" 2>/dev/null; then
      echo "== the EMULATOR PROCESS DIED mid-wait (pid $EMULATOR_PID) ==" >&2
      tail -40 "$OUT/emulator.log" >&2 2>/dev/null || true
      return 2
    fi
    echo "   wait:     '${pattern}' from pid $pid (attempt $attempt)" >&2
    attempt=$((attempt + 1))
    sleep 2
  done
  return 1
}

RC=0
if ! wait_for_log "first frame of this gpu generation presented"; then RC=$?; fi
if [ "$RC" -ne 0 ]; then
  timeout 30 "$ADB" logcat -d >"$OUT/logcat-first-frame-failed.txt" 2>/dev/null || true
  tail -80 "$OUT/logcat-first-frame-failed.txt" >&2 || true
  if [ "$RC" -eq 2 ]; then
    # The EMULATOR died (GPU emulation), not the app: the launch markers up
    # to here (am-start ok, android_main entered) PASSED, and painting is
    # unassertable on a dead emulator. Red, with the precise verdict.
    fail "the EMULATOR process died mid-launch (its GPU emulation segfaulted during the app's first-frame init) — the app's own launch markers passed; painting on real devices stays the owner-device gate"
  fi
  fail "no 'first frame ... presented' marker — the surface never painted"
fi
echo "   launch:   android_main entered + first frame presented"

# First-run: the onboarding sheet rides the Library. Capture it, then dismiss it
# with BACK (the compact stack treats back-on-the-intro as Skip and persists
# first_run_seen), and relaunch if that back instead left the root.
sleep 2
"$ADB" exec-out screencap -p >"$OUT/launch-onboarding.png"
timeout 45 "$ADB" shell uiautomator dump /sdcard/window-launch.xml >/dev/null 2>&1 || true
"$ADB" pull /sdcard/window-launch.xml "$OUT/window-launch.xml" >/dev/null 2>&1 || true

# Remember the first process before BACK: the relaunch's proof is a marker from
# a DIFFERENT pid, whose entry counter reads #1 again (a fresh process).
FIRST_LAUNCH="$(latest_main_entry || true)"
[ -n "$FIRST_LAUNCH" ] || fail "no 'android_main entered' line to read the first pid from"
FIRST_PID="${FIRST_LAUNCH%% *}"
echo "   launch:   first process pid $FIRST_PID"

wait_for_library_ready || fail "the compact Library never drew on the first launch (no 'compact library surface: ready' marker within ${LIBRARY_TIMEOUT}s) — the compact host never came up"

"$ADB" shell input keyevent 4
# --- M13I-b layer 1: BACK finishes the Activity and the process must leave ----
# The old code returned from the loop and lingered, which is the state that made
# the next launch's EventLoop build fail and the JNI side crash. A finish is
# only clean when the package has no process at all.
if ! wait_for_process_exit; then
  # The first BACK may have dismissed a sheet instead of finishing the root.
  echo "   finish:   still alive after BACK — pressing BACK once more (sheet → root)"
  "$ADB" shell input keyevent 4
  if ! wait_for_process_exit; then
    timeout 30 "$ADB" logcat -d >"$OUT/logcat-back-failed.txt" 2>/dev/null || true
    "$ADB" shell dumpsys activity exit-info "$PACKAGE" >"$OUT/exit-info-back.txt" 2>&1 || true
    fail "the app process was still alive ${FINISH_TIMEOUT}s after BACK — the finish path did not end the process (the state that leaves a dead EventLoop for the next launch)"
  fi
fi
echo "   finish:   process exited after BACK (clean finish path)"

"$ADB" shell am start -W -n "$ACTIVITY" >"$OUT/am-start-library.txt" 2>&1 || true

# (a) the SECOND launch: a fresh pid, and the per-process entry counter at #1.
RELAUNCH=""
RELAUNCH="$(wait_for_fresh_launch)" || {
  RC=$?
  timeout 30 "$ADB" logcat -d >"$OUT/logcat-relaunch-failed.txt" 2>/dev/null || true
  "$ADB" shell dumpsys activity exit-info "$PACKAGE" >"$OUT/exit-info-relaunch.txt" 2>&1 || true
  if grep -qE "reason=2 \(SIGNALED\)" "$OUT/exit-info-relaunch.txt"; then
    cat "$OUT/exit-info-relaunch.txt" >&2
    fail "the relaunch died on a native SIGNAL (ApplicationExitInfo reason=2 (SIGNALED); status=11 in the CI repro was SIGSEGV) — the one-loop-per-process contract regressed: the finish path must exit the process, and the next launch must start a fresh one"
  fi
  if [ "$RC" -eq 2 ]; then
    fail "the EMULATOR process died during the relaunch wait — the app's own relaunch markers are unassertable (see $OUT/emulator.log)"
  fi
  fail "no fresh-process 'android_main entered' marker within ${LAUNCH_TIMEOUT}s of the relaunch (see $OUT/logcat-relaunch-failed.txt, $OUT/exit-info-relaunch.txt)"
}
NEW_PID="${RELAUNCH%% *}"
NEW_ENTRY="${RELAUNCH##* }"
[ "$NEW_ENTRY" = "1" ] || fail "the relaunch logged entry #$NEW_ENTRY in pid $NEW_PID — a fresh process resets the counter to #1; entry #2 means android_main ran again in the old process (the EventLoop-can't-be-recreated shape)"
echo "   relaunch: pid $NEW_PID (was $FIRST_PID), entry #$NEW_ENTRY (fresh process)"

# (b) the first frame of the new generation, written by the NEW process.
wait_for_pid_log "$NEW_PID" "first frame of this gpu generation presented" || {
  RC=$?
  timeout 30 "$ADB" logcat -d >"$OUT/logcat-relaunch-frame-failed.txt" 2>/dev/null || true
  if [ "$RC" -eq 2 ]; then
    fail "the EMULATOR process died while waiting for the new process's first frame (see $OUT/emulator.log)"
  fi
  fail "the fresh process (pid $NEW_PID) never logged 'first frame of this gpu generation presented' — the relaunch started but never painted (see $OUT/logcat-relaunch-frame-failed.txt)"
}
echo "   relaunch: first frame presented by pid $NEW_PID"

# The guard's own marker must never appear in a passing run: it means a launch
# entered android_main in a process that already had a loop — the exact state
# the finish-path exit exists to prevent.
APP_LOG_ALL="$(app_log)"
case "$APP_LOG_ALL" in
  *"EventLoop can't be recreated"*)
    printf '%s\n' "$APP_LOG_ALL" | grep -nF "EventLoop can't be recreated" >&2 || true
    fail "'EventLoop can't be recreated' was logged — android_main ran again in a process that already had an event loop; the finish path must exit the process"
    ;;
esac

wait_for_library_ready || fail "the compact Library never drew (no 'compact library surface: ready' marker within ${LIBRARY_TIMEOUT}s) — the compact host never came up"

sleep 3

"$ADB" exec-out screencap -p >"$OUT/library.png"
timeout 45 "$ADB" shell uiautomator dump /sdcard/window-library.xml >/dev/null 2>&1 || true
"$ADB" pull /sdcard/window-library.xml "$OUT/window-library.xml" >/dev/null 2>&1 || true
"$ADB" shell dumpsys window >"$OUT/dumpsys-window.txt" 2>&1 || true
"$ADB" shell dumpsys activity activities >"$OUT/dumpsys-activities.txt" 2>&1 || true
# Why a process died (crash/ANR/LMKD), if it did: the platform's own record —
# directly relevant to the owner's "opens then closes" report.
"$ADB" shell dumpsys activity exit-info "$PACKAGE" >"$OUT/exit-info.txt" 2>&1 || true
"$ADB" shell dumpsys package "$PACKAGE" | grep -E "primaryCpuAbi|versionName|versionCode" >"$OUT/package.txt" || true
timeout 30 "$ADB" logcat -d >"$OUT/logcat-launch.txt" 2>/dev/null || true
timeout 30 "$ADB" logcat -d -s SuramakSpike:I >"$OUT/logcat-app.txt" 2>/dev/null || true

# --- assertions ---------------------------------------------------------------
echo "== assertions =="
if grep -qE "FATAL EXCEPTION|ANR in $PACKAGE|Force finishing activity $PACKAGE" "$OUT/logcat-launch.txt"; then
  grep -nE "FATAL EXCEPTION|ANR in $PACKAGE|Force finishing activity $PACKAGE" \
    "$OUT/logcat-launch.txt" >&2 || true
  fail "a fatal exception/ANR was logged for $PACKAGE"
fi

# (d) the platform's exit record: no SIGNALED death for the package. reason=2
# is ApplicationExitInfo.REASON_SIGNALED (status=11 was SIGSEGV in the CI
# repro). This check runs BEFORE the focus assertion so a native death is named
# as one instead of "the app did not come to the front".
if grep -qE "reason=2 \(SIGNALED\)" "$OUT/exit-info.txt"; then
  echo "== ApplicationExitInfo: the app died on a SIGNAL ==" >&2
  cat "$OUT/exit-info.txt" >&2
  fail "the app died on a native SIGNAL (ApplicationExitInfo reason=2 (SIGNALED); status=11 in the CI repro was SIGSEGV) — the one-loop-per-process contract regressed"
fi
if grep -qE "reason=1 \(EXIT_SELF\)" "$OUT/exit-info.txt"; then
  echo "   exit-info: reason=1 (EXIT_SELF) recorded — the finish path's clean exit"
else
  echo "   exit-info: no SIGNALED death for $PACKAGE"
fi

FOCUS="$(grep -E "mCurrentFocus|mFocusedApp" "$OUT/dumpsys-window.txt" | head -4 || true)"
# dumpsys prints the component as `app.suramak.spike/app.suramak.spike.MainActivity`
# (the shorthand `.MainActivity` is what `am start` accepts, not what it prints).
case "$FOCUS" in
  *"$PACKAGE"*"MainActivity"*) ;;
  *)
    printf '%s\n' "$FOCUS" >&2
    fail "the focused window is not $PACKAGE/.MainActivity — the app did not come to the front"
    ;;
esac
echo "   focus:    $PACKAGE/.MainActivity"

if [ -s "$OUT/window-library.xml" ]; then
  if grep -qiE "has stopped|isn't responding|Application Not Responding" "$OUT/window-library.xml"; then
    fail "the uiautomator dump carries a crash/ANR dialog"
  fi
  grep -q "$PACKAGE" "$OUT/window-library.xml" \
    || fail "the uiautomator dump does not name $PACKAGE"
fi

# The screencap gate: the compact Library's painted surface, not a blank frame
# and not the launcher. Thresholds are calibrated on tests/ui-compact/shots.
python3 tests/android-smoke/inspect_screencap.py \
  --shot "$OUT/library.png" \
  --baseline "$OUT/launcher.png" \
  --json "$OUT/screencap.json" || fail "the Library screencap failed inspection (see $OUT/screencap.json)"

# The app's own crash record (Rust panics + Java exceptions write this file).
"$ADB" pull "/sdcard/Android/data/$PACKAGE/files/spike-last-crash.txt" "$OUT/spike-last-crash.txt" >/dev/null 2>&1 || true
if [ -s "$OUT/spike-last-crash.txt" ]; then
  echo "   crash file present (context; a fresh install should have none):" >&2
  head -30 "$OUT/spike-last-crash.txt" >&2 || true
fi

echo "   library:  rendered (screencap inspected)"

# --- M19B: the model table tells the truth on a fresh install ----------------
# The owner-reported defect: a phone that never downloaded turbo-q5-0 showed it
# as "In use". egui paints pixels, so uiautomator cannot read the row text; the
# shell's own SURAMAK-MODEL marker (one line per state change) is the witness.
# The install is fresh (`-wipe-data`), so models/ must be empty for the
# assertion to mean anything.
echo "== M19B: no 'In use' without a verified model =="
MODELS_LISTING="$("$ADB" shell ls "/sdcard/Android/data/$PACKAGE/files/models" 2>/dev/null | tr -d '\r' || true)"
if [ -n "$MODELS_LISTING" ]; then
  printf '%s\n' "$MODELS_LISTING" >&2
  fail "the fresh install already has files in files/models — the 'no In use' assertion would be vacuous"
fi
grep -F "SURAMAK-MODEL" "$OUT/logcat-app.txt" >"$OUT/model-log.txt" || true
[ -s "$OUT/model-log.txt" ] \
  || fail "the shell logged no SURAMAK-MODEL lines — the M19B row-state marker did not run (see $OUT/logcat-app.txt)"
python3 - "$OUT/model-log.txt" <<'PY' || fail "the model table is not honest on a fresh install (see $OUT/model-log.txt and $OUT/logcat-app.txt)"
import sys

lines = [line for line in open(sys.argv[1], encoding="utf-8", errors="replace")]
rows = [line for line in lines if "row=" in line]
headers = [line for line in lines if "header=\"" in line]
if not rows:
    sys.exit("no SURAMAK-MODEL row lines")
if not headers:
    sys.exit("no SURAMAK-MODEL header line")
in_use = [line.strip() for line in rows if "in_use=1" in line or "state=in_use" in line]
if in_use:
    sys.exit("a row claims 'In use' on a fresh install with no model files: " + "; ".join(in_use))
if "not downloaded" not in headers[-1]:
    sys.exit(f"the header row does not carry 'not downloaded': {headers[-1].strip()!r}")
states = sorted({part.split()[0] for line in rows for part in line.split() if part.startswith("state=")})
print(f"   model:    header={headers[-1].strip()}")
print(f"   model:    no row says 'In use'; row states seen: {', '.join(states)}")
PY

# --- M22B: share → micro-sheet → voice-note notification ---------------------
#
# The step is gated on a verified speech model: a fresh `-wipe-data` emulator
# has none, and downloading one here would put a network dependency in the
# launch gate. Set SURAMAK_SMOKE_MODEL=/path/to/ggml-tiny.bin to push one; CI
# leaves it unset and the step asserts the sheet + the honest skip instead.
echo "== M22B: share a voice note → micro-sheet → notification =="
VOICE_DIR="/sdcard/Android/data/$PACKAGE/files"
VOICE_FIXTURE="$VOICE_DIR/smoke-voice-note.wav"
VOICE_LONG="$VOICE_DIR/smoke-voice-note-long.wav"
mkdir -p "$OUT"
"$ADB" push "$REPO_ROOT/fixtures/audio/en.wav" "$VOICE_FIXTURE" >/dev/null
# A 90 s loop of the same sentence: long enough to observe the progress
# notification and cancel it on a fast emulator.
python3 - "$REPO_ROOT/fixtures/audio/en.wav" "$OUT/smoke-voice-note-long.wav" <<'PY'
import sys, wave
source, dest = sys.argv[1], sys.argv[2]
with wave.open(source, "rb") as src:
    params = src.getparams()
    frames = src.readframes(src.getnframes())
with wave.open(dest, "wb") as out:
    out.setparams(params)
    for _ in range(10):
        out.writeframes(frames)
PY
"$ADB" push "$OUT/smoke-voice-note-long.wav" "$VOICE_LONG" >/dev/null

MODEL_FILE="$VOICE_DIR/models/ggml-tiny.bin"
MODEL_PRESENT="$("$ADB" shell "[ -f '$MODEL_FILE' ] && echo yes || echo no" | tr -d '\r')"
if [ "$MODEL_PRESENT" != "yes" ] && [ -n "${SURAMAK_SMOKE_MODEL:-}" ] && [ -f "${SURAMAK_SMOKE_MODEL:-}" ]; then
  "$ADB" shell mkdir -p "$VOICE_DIR/models" >/dev/null 2>&1 || true
  "$ADB" push "$SURAMAK_SMOKE_MODEL" "$MODEL_FILE" >/dev/null
  MODEL_PRESENT="yes"
fi

share_voice_note() {
  local file="$1" language="$2" tag="$3"
  "$ADB" shell am start \
    -a android.intent.action.SEND -t audio/wav \
    --eu android.intent.extra.STREAM "file://$VOICE_DIR/$file" \
    --es language "$language" \
    -n "$ACTIVITY" >"$OUT/voice-note-$tag-share.txt" 2>&1 \
    || fail "am start (voice note $tag) failed"
}

# The sheet paints for every share, model or not. The shell's own marker is the
# witness: egui text is invisible to uiautomator.
if [ "$MODEL_PRESENT" != "yes" ]; then
  "$ADB" logcat -c 2>/dev/null || true
  share_voice_note "smoke-voice-note.wav" "en" "ready"
  wait_for_log "voice note sheet: shown" \
    || fail "the share did not open the voice-note micro-sheet (no 'voice note sheet: shown' marker)"
  echo "   sheet:    shown for an audio share"
  echo "M22B-SKIP: no speech model at $MODEL_FILE — the micro-sheet was asserted; the"
  echo "           notification step needs a model (set SURAMAK_SMOKE_MODEL to a"
  echo "           ggml-tiny.bin to run it). This is a gate, not a pass for the notification."
  # BACK is *attempted*, never required. On this emulator the key stops being
  # delivered to the app while the share opened with FLAG_ACTIVITY_NEW_TASK and
  # an IME window is in front (three presses produced zero events in
  # 43216992141/43219365426: no dispatchKeyEvent, no OnBackInvoked log, while
  # the same key reaches the app at the library root). The dismissal contract
  # itself is proven in Rust — compact_host's
  # `back_on_the_share_sheet_tells_the_activity_the_share_was_refused` asserts
  # the reply carries `voice_note: dismissed` — so this step reports what the
  # emulator did instead of pretending a key it never saw proves the product.
  "$ADB" shell input keyevent 4
  sleep 1
  if app_log | grep -q "voice note: the offer was dismissed"; then
    echo "   sheet:    dismissed by BACK; nothing was scheduled"
  else
    echo "   sheet:    NOTE — this emulator did not deliver BACK to the app"
    echo "             (no key event logged; the reply-level dismissal is unit-tested)"
  fi
  # Leave a live app behind: the steps after this one drive the Library, and a
  # force-stopped process makes every tap dead (run 43221792050 failed exactly
  # there). Relaunch and wait for the real surface marker, not a sleep.
  "$ADB" shell am force-stop "$PACKAGE" 2>/dev/null || true
  "$ADB" logcat -c 2>/dev/null || true
  "$ADB" shell am start -W -n "$ACTIVITY" >"$OUT/am-start-after-share.txt" 2>&1 || true
  wait_for_library_ready || fail "the app did not come back after the share step (no library marker)"
else
  # Run 1 — cancel while it is transcribing. The key is the service's own log
  # line, never a guess; Cancel is the same broadcast the notification uses.
  "$ADB" logcat -c 2>/dev/null || true
  share_voice_note "smoke-voice-note-long.wav" "en" "cancel"
  wait_for_log "voice note sheet: shown" \
    || fail "the share did not open the voice-note micro-sheet"
  echo "   sheet:    shown for an audio share"
  KEY=""
  DEADLINE=$((SECONDS + 120))
  while [ "$SECONDS" -lt "$DEADLINE" ]; do
    captured="$(timeout 15 "$ADB" logcat -d -s SuramakTranscribe:I 2>/dev/null || true)"
    KEY="$(printf '%s\n' "$captured" | sed -nE 's/.*voice-note job key=([^ ]+) percent=([0-9]+).*/\1/p' | tail -1)"
    [ -n "$KEY" ] && break
    sleep 1
  done
  [ -n "$KEY" ] || fail "the progress notification never logged its job key — the cancel step cannot name the job"
  "$ADB" shell dumpsys notification --noredact >"$OUT/voice-note-progress.txt" 2>&1 || true
  grep -q "Transcribing…" "$OUT/voice-note-progress.txt" \
    || fail "no 'Transcribing…' voice-note notification while the job ran"
  "$ADB" shell am broadcast -a app.suramak.transcribe.CANCEL \
    -n "$PACKAGE/.transcribe.CancelTranscriptionReceiver" \
    --es app.suramak.transcribe.KEY "$KEY" >"$OUT/voice-note-cancel.txt" 2>&1 \
    || fail "the Cancel broadcast was refused"
  CANCEL_DEADLINE=$((SECONDS + 30))
  while [ "$SECONDS" -lt "$CANCEL_DEADLINE" ]; do
    "$ADB" shell dumpsys notification --noredact >"$OUT/voice-note-cancelled.txt" 2>&1 || true
    if grep -q "Transcription cancelled" "$OUT/voice-note-cancelled.txt"; then break; fi
    sleep 1
  done
  grep -q "Transcription cancelled" "$OUT/voice-note-cancelled.txt" \
    || fail "the voice-note job did not report Cancelled after the notification's Cancel"
  echo "   cancel:   job $KEY cancelled from the notification"

  # Run 2 — the finished transcript. The 9 s fixture says the fox sentence;
  # its first segment must appear in the notification body.
  share_voice_note "smoke-voice-note.wav" "en" "ready"
  READY_DEADLINE=$((SECONDS + 180))
  while [ "$SECONDS" -lt "$READY_DEADLINE" ]; do
    "$ADB" shell dumpsys notification --noredact >"$OUT/voice-note-ready.txt" 2>&1 || true
    if grep -q "Transcript ready" "$OUT/voice-note-ready.txt"; then break; fi
    sleep 2
  done
  grep -q "Transcript ready" "$OUT/voice-note-ready.txt" \
    || fail "the finished voice note never posted a 'Transcript ready' notification"
  grep -q "quick brown fox" "$OUT/voice-note-ready.txt" \
    || fail "the notification does not carry the first segment's words ('quick brown fox')"
  echo "   finish:   'Transcript ready' notification carries the transcript"
  echo "   cancel:   the notification's Cancel path was exercised on run 1"
fi

# --- M19A: the Settings list scrolls, taps land after a swipe, and the DEVICE
# --- card clears the gesture pill --------------------------------------------
#
# Every assertion below is pixel- or log-derived: the compact surface is ONE
# Android window, so uiautomator sees a single node and cannot describe the
# list moving. The witnesses are (a) a screencap diff before/after a swipe,
# with a still-frame negative control and the status-bar band cropped out of
# both, (b) the shell's own route markers for "a tap landed", and (c) the
# shell's SURAMAK-LAYOUT resting line for where the QUIT row — the list's last
# control — sits relative to the viewport (the gesture inset).
echo "== M19A: Settings scroll, tap after a swipe, gesture-pill clearance =="

# Geometry in physical pixels: input tap/swipe take pixels, the shell logs
# egui points. Their ratio is Step 0's second answer.
SCREEN_W="$("$ADB" shell wm size | tr -d '\r' | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\1/p' | tail -1)"
SCREEN_H="$("$ADB" shell wm size | tr -d '\r' | sed -n 's/.*: \([0-9]*\)x\([0-9]*\).*/\2/p' | tail -1)"
DENSITY="$("$ADB" shell wm density | tr -d '\r' | sed -n 's/.*: \([0-9]*\).*/\1/p' | tail -1)"
[ -n "$SCREEN_W" ] && [ -n "$SCREEN_H" ] && [ -n "$DENSITY" ] \
  || fail "could not read the screen geometry (wm size='${SCREEN_W}x${SCREEN_H}' density='${DENSITY}')"
echo "   screen:   ${SCREEN_W}x${SCREEN_H} px, density ${DENSITY} dpi"

dp() { awk -v value="$1" -v density="$DENSITY" 'BEGIN { printf "%d", value * density / 160 }'; }

# The host clock in Unix milliseconds, the unit the shell's `ts=` uses. GNU
# date's `%N` is nanoseconds on glibc whatever the width modifier says, so the
# first 13 digits are seconds + milliseconds.
now_ms() { date +%s%3N | cut -c1-13; }

route_is() {
  local lines
  lines="$(app_log)"
  case "$lines" in
    *"compact route: $1"*) return 0 ;;
  esac
  return 1
}

# Taps the app bar at `x` on a small ladder of heights and stops at the first
# one that changes the route: the bar's centre is `status bar + 28 dp`, and the
# status bar is 0–48 dp depending on the cutout. A fixed y would be a coin
# flip on an unknown skin.
tap_and_wait_route() {
  local x="$1" route="$2" y attempt=0 override="${3:-}"
  local candidates
  candidates="${override:-$(dp 46) $(dp 70) $(dp 30)}"
  for y in $candidates; do
    attempt=$((attempt + 1))
    "$ADB" shell input tap "$x" "$y"
    local deadline=$((SECONDS + 8))
    while [ "$SECONDS" -lt "$deadline" ]; do
      if route_is "$route"; then
        echo "   tap:      (${x}, ${y}) → ${route}"
        return 0
      fi
      sleep 1
    done
    echo "   tap:      no ${route} route at (${x}, ${y}) (attempt ${attempt})"
  done
  return 1
}

enter_settings() {
  # The gear's REAL position, as the shell logged it when it drew the Library
  # (`compact library: settings at X,Y`, device pixels). The dp guesses below
  # stopped being true when the header took the new type scale: run 43223493395
  # tapped 13 px above a gear whose centre had moved to ~196 px (66 dp).
  local point
  point="$(timeout 20 "$ADB" logcat -d 2>/dev/null | sed -n 's/.*compact library: settings at \([0-9]*\),\([0-9]*\).*/\1 \2/p' | tail -1)"
  if [ -n "$point" ]; then
    set -- $point
    if tap_and_wait_route "$1" Settings "$2"; then return 0; fi
  fi
  # Fallbacks, in dp from the top edge, for a build that predates the marker.
  tap_and_wait_route "$((SCREEN_W - $(dp 40)))" Settings
}

# Clear logcat so a route marker seen below is one THIS tap produced (the first
# launch already logged "compact route: Library").
"$ADB" logcat -c 2>/dev/null || true

enter_settings || {
  timeout 30 "$ADB" logcat -d >"$OUT/logcat-settings-failed.txt" 2>/dev/null || true
  fail "a tap on the Library's Settings action never opened Settings — taps are dead, so the M19A steps cannot run (see $OUT/logcat-settings-failed.txt)"
}
sleep 1

# --- the status-bar band to crop out of every diff (M19A2) -------------------
# Two captures of a still screen flake when the status bar's clock rolls over or
# its battery/network icons redraw. The band is measured from the app's OWN top
# inset (`SURAMAK-LAYOUT inset_top`, in points) times the frame's
# pixels-per-point (`SURAMAK-INPUT ppp`), with a 24 dp floor for the frames
# before the Kotlin inset listener has fired — the app never draws there.
last_layout_field() {
  app_log | grep -F "SURAMAK-LAYOUT" | tail -1 \
    | sed -n "s/.*[[:space:]]$1=\(-\{0,1\}[0-9.]*\).*/\1/p" || true
}
INSET_TOP_PT="$(last_layout_field inset_top)"
PPP="$(app_log | grep -F "SURAMAK-INPUT" | tail -1 \
  | sed -n 's/.*ppp=\([0-9.]*\).*/\1/p' || true)"
if [ -z "$PPP" ]; then
  # egui's ppp on Android is the display density over 160, and the debug APK's
  # first SURAMAK-INPUT line may not have been written yet.
  PPP="$(awk -v density="$DENSITY" 'BEGIN { printf "%.3f", density / 160 }')"
fi
[ -n "$INSET_TOP_PT" ] || INSET_TOP_PT=0
CROP_TOP="$(awk -v top="$INSET_TOP_PT" -v ppp="$PPP" -v density="$DENSITY" 'BEGIN {
  px = top * ppp;
  floor = 24 * density / 160;   # 24 dp: the shortest status bar Android ships
  if (px < floor) px = floor;
  printf "%d", px + 2;          # +2 px for the 1-px shadow at the bar edge
}')"
[ "$CROP_TOP" -gt 0 ] || fail "the status-bar crop band came out empty (inset_top=${INSET_TOP_PT} pt, ppp=${PPP})"
[ "$CROP_TOP" -lt "$((SCREEN_H / 4))" ] \
  || fail "the status-bar crop (${CROP_TOP} px) is over a quarter of the ${SCREEN_H} px screen — refusing to crop the evidence away"
echo "   crop:     status bar rows 0..${CROP_TOP} px (inset_top ${INSET_TOP_PT} pt × ppp ${PPP}) — cropped from both diff frames"

# --- the swipe: the list must move -------------------------------------------
"$ADB" exec-out screencap -p >"$OUT/settings-before.png"
sleep 0.4
"$ADB" exec-out screencap -p >"$OUT/settings-still.png"

# A thumb flick: 30% of the screen height, 250 ms, inside the list.
"$ADB" shell input swipe \
  "$((SCREEN_W / 2))" "$((SCREEN_H * 65 / 100))" \
  "$((SCREEN_W / 2))" "$((SCREEN_H * 35 / 100))" 250
sleep 1
"$ADB" exec-out screencap -p >"$OUT/settings-after-swipe.png"

# --- Step 0: the two answers, from the shell's raw-input log ------------------
# The debug APK this job builds has SURAMAK_INPUT_LOG on by default (see
# `compact::input::input_log_enabled`); the swipe above is the input.
app_log | grep -F "SURAMAK-INPUT" >"$OUT/input-log.txt" || true
[ -s "$OUT/input-log.txt" ] \
  || fail "the shell logged no SURAMAK-INPUT lines — the M19A instrumentation did not run (debug build / SURAMAK_INPUT_LOG)"
python3 - "$OUT/input-log.txt" <<'PY' || fail "Step 0's answers could not be read from $OUT/input-log.txt"
import re
import sys

lines = [line for line in open(sys.argv[1], encoding="utf-8", errors="replace") if "SURAMAK-INPUT" in line]
if not lines:
    sys.exit("no SURAMAK-INPUT lines")

buttons = sum("ptrbutton=1" in line for line in lines)
print(
    "   STEP0 pointer events: "
    + ("YES — the integration already translates touch (synth=0 on those frames)"
       if buttons else "NO — only raw Event::Touch arrives; the shell synthesises the pointer")
    + f" [{buttons}/{len(lines)} frames carried a PointerButton]"
)

touches = 0
for line in lines:
    screen = re.search(r"screen=([\d.]+)x([\d.]+)", line)
    ppp = re.search(r"ppp=([\d.]+)", line)
    if not screen:
        continue
    width, height = float(screen.group(1)), float(screen.group(2))
    for x, y in re.findall(r"@(-?[\d.]+),(-?[\d.]+)", line):
        touches += 1
        x, y = float(x), float(y)
        if not (-0.5 <= x <= width + 0.5 and -0.5 <= y <= height + 0.5):
            sys.exit(
                f"touch ({x},{y}) is outside the {width}x{height} pt screen the shell reports "
                f"(ppp={ppp.group(1) if ppp else '?'}) — the coordinates are NOT egui points"
            )
if touches == 0:
    sys.exit("no touch positions in the input log")
print(
    f"   STEP0 units:          YES — every touch position is inside the {width:.1f}x{height:.1f} "
    "pt screen rect (already divided by pixels-per-point)"
)
PY

# --- the two pixel assertions ------------------------------------------------
# Negative control first: two captures of a still screen must be identical, or
# the diff below proves nothing. Both frames are cropped to the status-bar band's
# edge first (M19A2): the clock and the battery icon are the system's, not the
# app's, and a minute rollover must not be able to fail this control.
python3 tests/android-smoke/inspect_screencap.py \
  --diff "$OUT/settings-still.png" "$OUT/settings-before.png" \
  --crop-top "$CROP_TOP" --max-mean-diff 0.05 --json "$OUT/settings-still-diff.json" \
  || fail "two captures of a still Settings screen differ below the status bar — the screencap diff is not a stable witness (see $OUT/settings-still-diff.json)"

python3 tests/android-smoke/inspect_screencap.py \
  --diff "$OUT/settings-before.png" "$OUT/settings-after-swipe.png" \
  --crop-top "$CROP_TOP" --min-mean-diff 0.4 --json "$OUT/settings-swipe-diff.json" \
  || fail "the Settings list did not move after a swipe (see $OUT/settings-swipe-diff.json and the two shots) — the dead-scroll regression is back"

# --- a tap after the swipe must still fire -----------------------------------
# BACK is at a fixed spot whatever the list's offset: if the gesture left the
# pointer wedged (or the scroll swallowed the next tap), the route never
# changes and this fails.
tap_and_wait_route "$(dp 28)" Library \
  || fail "a tap on Settings' back chevron after the swipe did not return to the Library — taps stop landing after a scroll"
echo "   tap:      a tap fired after the swipe"

# --- bottom clearance: the QUIT row above the gesture area -------------------
# M19A2: the probe measures the list's real last control. Diagnostics and the
# Quit row follow the DEVICE card, so at full scroll Quit is what sits under the
# pill — the M19A probe measured the DEVICE note, a middle element that cleared
# the band even when Quit was clipped.
enter_settings || fail "could not re-enter Settings for the bottom-clearance check"
sleep 1
# Scroll to the end (the list clamps at its own bottom; extra swipes are no-ops).
# The shell logs a line on every geometry change (4 Hz ceiling) and one more
# once the offset has been still for 300 ms; `LAST_SWIPE_MS` is the host clock
# reading taken just before the final swipe, which the line's own `ts=` must
# beat.
# A resting line is proven by the HARNESS: the last SURAMAK-LAYOUT line must be
# IDENTICAL 600 ms later (no geometry change). This does not trust the app's
# own settle timer, which only fires when a frame lands after the motion stops —
# on a reactive loop that can be a coin flip, and one run had no settled line at
# all. The app logs on every geometry change, so "same line twice" IS the rest.
resting_layout_line() {
  local first second attempt
  for attempt in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16; do
    first="$(app_log | grep -F "SURAMAK-LAYOUT" | tail -1 || true)"
    if [ -n "$first" ]; then
      sleep 0.6
      second="$(app_log | grep -F "SURAMAK-LAYOUT" | tail -1 || true)"
      if [ "$first" = "$second" ]; then
        printf '%s\n' "$first"
        return 0
      fi
    else
      sleep 0.6
    fi
  done
  return 1
}

LINE=""
for round in 1 2 3 4; do
  for _ in 1 2 3; do
    "$ADB" shell input swipe \
      "$((SCREEN_W / 2))" "$((SCREEN_H * 72 / 100))" \
      "$((SCREEN_W / 2))" "$((SCREEN_H * 28 / 100))" 200
    sleep 0.3
  done
  if LINE="$(resting_layout_line)"; then
    break
  fi
  echo "   scroll:   list still moving after round $round — swiping again" >&2
done
[ -n "$LINE" ] || fail "no resting SURAMAK-LAYOUT line after the swipes — the geometry never stopped changing (see the app log)"
printf '%s\n' "$LINE" >"$OUT/layout.txt"
"$ADB" exec-out screencap -p >"$OUT/settings-bottom.png"

python3 - "$OUT/layout.txt" "0" <<'PY' || fail "the Quit row does not clear the gesture area (see $OUT/layout.txt and $OUT/settings-bottom.png)"
import re
import sys

line = open(sys.argv[1], encoding="utf-8", errors="replace").read().strip()
values = dict(re.findall(r"([a-z_]+)=(-?[\d.]+)", line))
required = (
    "quit_bottom",
    "clearance",
    "viewport_bottom",
    "inset_bottom",
    "inset_top",
    "settled",
    "ts",
)
missing = [field for field in required if field not in values]
if missing:
    sys.exit(f"the SURAMAK-LAYOUT line is missing {missing}: {line!r}")

quit_bottom = float(values["quit_bottom"])
clearance = float(values["clearance"])
viewport_bottom = float(values["viewport_bottom"])
inset_bottom = float(values["inset_bottom"])
inset_top = float(values["inset_top"])
settled = int(float(values["settled"]))
ts = float(values["ts"])
swipe_ms = float(sys.argv[2] or 0)

print(
    f"   clearance: QUIT ends at {quit_bottom:.1f} pt + {clearance:.1f} pt padding; viewport "
    f"ends at {viewport_bottom:.1f} (inset_bottom {inset_bottom:.1f}, inset_top "
    f"{inset_top:.1f}); settled={settled}, ts={ts:.0f}"
)
# `settled` is the app's own timer; the harness already proved the line is the
# resting one (identical 600 ms later), so it is reported, not required.
if settled != 1:
    print("   note:     the app's settle timer did not fire; rest proven by the harness")
if inset_bottom <= 0.0:
    sys.exit(
        "inset_bottom is 0 on the gesture-nav AVD — the WindowInsets never reached the "
        "shell, so every clearance number above is measured against a guess"
    )
# The row must be fully ABOVE the viewport's bottom edge, which already
# excludes the gesture inset: that is what "not clipped by the pill" means.
# Its trailing padding may legitimately scroll past that edge — demanding the
# padding itself be visible (quit_bottom + clearance <= viewport_bottom) failed
# a healthy layout by 20 pt, because at max scroll the content bottom aligns
# with the viewport and the padding is exactly what hangs off it.
if quit_bottom > viewport_bottom + 0.5:
    sys.exit(
        f"the Quit row ({quit_bottom:.1f} pt) runs past the viewport bottom "
        f"({viewport_bottom:.1f} pt) at rest — it is clipped by the gesture area"
    )
if clearance + 0.5 < inset_bottom:
    sys.exit(
        f"the list's trailing clearance ({clearance:.1f} pt) is smaller than the gesture "
        f"inset ({inset_bottom:.1f} pt) — the last row has no room above the pill"
    )
print("   clearance: the Quit row sits fully above the gesture area at rest")
PY

tap_and_wait_route "$(dp 28)" Library || fail "could not return to the Library after the clearance check"

# --- monkey canary ------------------------------------------------------------
# Bounded, touch/motion only: system keys, app switching and trackball events
# are what turn a canary into a launcher tour. ~10 s wall at the default
# 30 x 250 ms (plus monkey's own startup).
echo "== monkey canary ($MONKEY_EVENTS events) =="
# Clear logcat first: the crash scan below must look at the MONKEY WINDOW only.
# The earlier finish-path exit legitimately logs "Process ... has died", and a
# whole-run scan once read that as a monkey crash (the app had exited cleanly
# through its own back/quit path — exit-info said reason=1 EXIT_SELF).
"$ADB" logcat -c 2>/dev/null || true
set +e
"$ADB" shell monkey -p "$PACKAGE" -v \
  --throttle 250 \
  --pct-touch 55 --pct-motion 45 \
  --pct-trackball 0 --pct-nav 0 --pct-majornav 0 --pct-syskeys 0 --pct-appswitch 0 --pct-anyevent 0 \
  "$MONKEY_EVENTS" >"$OUT/monkey.txt" 2>&1
MONKEY_RC=$?
set -e
tail -6 "$OUT/monkey.txt" || true
sleep 2
"$ADB" exec-out screencap -p >"$OUT/after-monkey.png" || true
timeout 30 "$ADB" logcat -d >"$OUT/logcat-full.txt" 2>/dev/null || true
"$ADB" shell dumpsys activity exit-info "$PACKAGE" >"$OUT/exit-info.txt" 2>&1 || true

if grep -qE "FATAL EXCEPTION|ANR in $PACKAGE|Force finishing activity $PACKAGE|Process $PACKAGE .* has died" "$OUT/logcat-full.txt"; then
  grep -nE "FATAL EXCEPTION|ANR in $PACKAGE|Force finishing activity $PACKAGE|Process $PACKAGE .* has died" \
    "$OUT/logcat-full.txt" >&2 || true
  fail "the monkey pass crashed $PACKAGE"
fi

PID="$("$ADB" shell pidof "$PACKAGE" 2>/dev/null | tr -d '\r' || true)"
if [ -z "$PID" ]; then
  # The compact Settings' Quit action ends the process on purpose; a monkey tap
  # that reaches it is a legitimate exit, not a crash (the log names it). A
  # BACK at the Library root is the same kind of exit: the finish path ends
  # the process deliberately (exit-info says reason=1 EXIT_SELF).
  if grep -q "SuramakQuit" "$OUT/logcat-full.txt"; then
    echo "   monkey:   the app exited through its own Quit path (not a crash)"
  elif grep -q "relaunch: .*ending the process (code 0)" "$OUT/logcat-full.txt" \
    && grep -qE "process=$PACKAGE .*reason=1 \(EXIT_SELF\)" "$OUT/exit-info.txt"; then
    echo "   monkey:   the app exited through the finish path (back at the Library root — not a crash)"
  else
    grep -E "process=$PACKAGE|reason=" "$OUT/exit-info.txt" >&2 || true
    fail "the app process is gone after the monkey pass with no Quit/finish marker (rc=$MONKEY_RC)"
  fi
else
  echo "   monkey:   process alive (pid $PID, rc=$MONKEY_RC)"
fi

echo
echo "SMOKE-OK: install → launch → compact Library painted → Settings swipes and taps → monkey survived"
echo "artifacts: $OUT (library.png, launch-onboarding.png, settings-before.png, settings-after-swipe.png,"
echo "           settings-bottom.png, settings-swipe-diff.json, input-log.txt, layout.txt, model-log.txt, logcat-full.txt)"
