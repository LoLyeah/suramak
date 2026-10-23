# WORKORDER — M13B6: spike v2.4 — rotation-kill + signal persistence (final shim item)

Tree: `main` at `710dbed`. **v2.3 field evidence (owner, Vivo X300 / Android 16,
share-log card):**

```
=== Suramak spike crash (session marker) ===
when:   2026-10-23 10:38:10Z (detected at launch)
where:  previous process
panic:  the session started at 2026-10-23 10:38:09Z · pid 20902 ·
        android 16 (API 36) · vivo V2515 — vanished without onStop.
        No Rust panic and no Java exception were recorded for it:
        the process was killed natively (SIGSEGV/SIGABRT/ANR-kill).
```

Decode: the dead process lived **~1 second** (10:38:09 → 10:38:10), **never
reached onStop**, no in-process record at all → **killed natively during
startup, right at/just after the rotate**. Owner's words: "rotate portrait to
landscape that crashes the app" — the FIRST time rotation was exercised (all
prior sessions stayed portrait), and the rotate happened near launch. This is a
native fault the Rust panic handler never sees, and the diag log ring (in
process memory) died with it — the only record is the NEXT launch's inference
(v2.3's detection did its part there).

## Hypothesis to verify against android-activity's source (cite lines)

The manifest deliberately does NOT swallow orientation (rotate = config change →
Activity destroyed + recreated). With GameActivity + android-activity 0.6.1 the
native loop pivots: android_main entry #1 must finish/suspend and a fresh entry
#2 starts in the same process. A SIGSEGV/SIGABRT within ~1 s of launch when
rotating says the destroy arrives **while the init path is still running** (the
surface/gpu/egui generation not yet built or half-built). Read
`~/.cargo/registry/src/*/android-activity-0.6.1/src/` (init.rs, lib.rs,
android_app.rs) for the destroy/config-change control flow and cite the exact
lines in the report; the fix must hold for BOTH race directions (destroy during
init, init during destroy) AND for a rotate mid-session (the healthy-rotation
path v2.2/2.3 never exercised: entry #2 on a live process).

## Fix — three layers

1. **Signal persistence**: install a POSIX handler for SIGSEGV/SIGABRT/SIGBUS in
   the spike's `android_main` bootstrap (lib.rs) that SYNCHRONOUSLY writes the
   diag log ring + the in-flight event + the signal number to the crash file
   (`spike-last-crash.txt` plain-text format, "native signal" variants) and then
   re-raises the signal so Android still records the tombstone. Constraint:
   async-signal-safety — pre-open the file descriptor (or write via a small
   buffer with only syscalls); no allocation, no mutex, no log macros in the
   handler path. Kotlin keeps reading the same files (CrashFiles already handles
   both + the external migration).
2. **Gate the destroy-during-init race**: same discipline as v2.2's suspend gate
   — a destroy/config-change arriving during gpu/egui init cannot fault; it
   marks the epoch ended and the next entry builds a fresh one. Unit-test the
   gate like the v2.2 suite did (an end-of-epoch build can never be adopted;
   a frame/surface event against an ended epoch is counted, not fatal).
3. **State survives**: probe text survives rotation AND process kill via the
   Android instance state — Kotlin `onSaveInstanceState` stores the probe
   string; the shim hands it to Rust over JNI on the next entry (`nativeRestore`
   style); Rust applies it to the SpikeState and logs `probe restored from
   instance state (N chars)`. Restore must be explicit and logged; the reset
   button still clears it. The real app persists to SQLite so this mechanism is
   for the spike's own probe, not the product path — keep it minimal.

## Version / packaging

versionCode **6**, versionName **0.3.0-m13b.2.4** (both version sites: the
gradle file + build-apk.sh if it carries the version), build via
`bash scripts/android/build-apk.sh` (all assertions must pass).

## Tests

Host-testeverything honest: the gate (init-during-destroy counted, not fatal),
the save/restore payload shape, the handler's synchronous write (spawn a child
process whose only job is: install the same handler, raise SIGSEGV, expect the
file on disk with the event text, exit via re-raise). `cargo fmt --all --check`;
`cargo clippy --workspace --all-targets -- -D warnings`; `cargo test
--workspace` (573 baseline, +new ones); `cargo ndk -t arm64-v8a --platform 29
clippy -p suramak-android-spike --all-targets -- -D warnings`;
`./gradlew :app:compileDebugKotlin --rerun-tasks` warning-free.

## Honest boundaries (report them)

- The native kill's removal is owner-device-only verifiable — the owner pass is
  (a) rotate within 1 s of launch ×3, (b) rotate mid-session ×3 (the healthy
  rotation path), expecting zero native kills and (for mid-session) probe text
  restored and logged.
- The signal handler's existence is proven by the subprocess test; its value is
  proven by the NEXT crash naming its state (or by none occurring).
- Whether the ANR/machinry-free Kill class (Vivo background kill) still happens
  is outside this fix — not our code, already documented (v2.3).

## Do NOT

- No new dependencies; no changes outside `crates/suramak-android-spike/` +
  `android/`; no version bump beyond versionCode 6 / `0.3.0-m13b.2.4`; no CI
  edits; no Gradle/AGP churn; no `git commit`/`push`; no sudo. Kotlin must
  compile warning-free per the repo's current gradle treatment.
- Report: `docs/reports/M13b6-spike-v2.4.md`.
