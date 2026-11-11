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
GPU_MODE="${SURAMAK_EMU_GPU:-angle_indirect}"
"$EMULATOR" -avd "$AVD_NAME" \
  -no-window -no-audio -no-boot-anim -no-snapshot -wipe-data \
  -gpu "$GPU_MODE" -accel on \
  -memory 2048 -partition-size 4096 \
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
  [ "$("$ADB" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r')" = "1" ] \
    && "$ADB" shell service check package 2>/dev/null | grep -q "found"
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
"$ADB" shell input keyevent 82 >/dev/null 2>&1 || true
"$ADB" shell wm dismiss-keyguard >/dev/null 2>&1 || true
sleep 2

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
    if timeout 20 "$ADB" logcat -d 2>/dev/null | grep -qF "$pattern"; then
      return 0
    fi
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
    if app_log | grep -qF "compact library surface: ready"; then
      echo "   library:  ready (compact host up, first real draw)"
      return 0
    fi
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
  app_log | awk -v pid="$1" '$3 == pid' | grep -qF "$2"
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
if app_log | grep -qF "EventLoop can't be recreated"; then
  app_log | grep -nF "EventLoop can't be recreated" >&2 || true
  fail "'EventLoop can't be recreated' was logged — android_main ran again in a process that already had an event loop; the finish path must exit the process"
fi

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

# --- monkey canary ------------------------------------------------------------
# Bounded, touch/motion only: system keys, app switching and trackball events
# are what turn a canary into a launcher tour. ~10 s wall at the default
# 30 x 250 ms (plus monkey's own startup).
echo "== monkey canary ($MONKEY_EVENTS events) =="
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

if grep -qE "FATAL EXCEPTION|ANR in $PACKAGE|Force finishing activity $PACKAGE|Process $PACKAGE .* has died" "$OUT/logcat-full.txt"; then
  grep -nE "FATAL EXCEPTION|ANR in $PACKAGE|Force finishing activity $PACKAGE|Process $PACKAGE .* has died" \
    "$OUT/logcat-full.txt" >&2 || true
  fail "the monkey pass crashed $PACKAGE"
fi

PID="$("$ADB" shell pidof "$PACKAGE" 2>/dev/null | tr -d '\r' || true)"
if [ -z "$PID" ]; then
  # The compact Settings' Quit action ends the process on purpose; a monkey tap
  # that reaches it is a legitimate exit, not a crash (the log names it).
  if grep -q "SuramakQuit" "$OUT/logcat-full.txt"; then
    echo "   monkey:   the app exited through its own Quit path (not a crash)"
  else
    fail "the app process is gone after the monkey pass with no Quit marker (rc=$MONKEY_RC)"
  fi
else
  echo "   monkey:   process alive (pid $PID, rc=$MONKEY_RC)"
fi

echo
echo "SMOKE-OK: install → launch → compact Library painted → monkey survived"
echo "artifacts: $OUT (library.png, launch-onboarding.png, screencap.json, logcat-full.txt)"
