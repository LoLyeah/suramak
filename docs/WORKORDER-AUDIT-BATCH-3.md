# WORKORDER — AUDIT-BATCH-3: close out audit pass 2

Tree: current `main` (post-`e71b47b`). **The spec for every item is `docs/reports/AGY-AUDIT-2-2026-10-21.md`** — each finding's section carries the file/lines, the analysis, and a recommended remediation. Read the report first, then the code at each cited location, and re-derive your fix from the code (the report is the checklist, not a copy-paste source: pass 2's own notes on `download.rs` proved the value of re-checking).

## The batch (in the report's recommended fix order)

**Mediums (must land):** SUR-MED-01 (zip bomb cap + PID-collision + prefix detection), SUR-MED-02 (`libc::_exit` bypassing destructors/WAL checkpoint), SUR-MED-03 (Parakeet FFI integer overflow), SUR-MED-04 (playback resampler div-by-zero/overflow), SUR-MED-05 (non-atomic settings.toml), SUR-MED-06 (insecure temp file in `scripts/ask_glm.py`), **SUR2-NEW-03** (in-process encoder-hide race), **SUR2-NEW-04** (non-cancellable LiveDecoder window during quit).

**Lows (land all 9 + the 3 from pass 2):** SUR-LOW-01..09 and SUR2-NEW-05 (standalone `upsert_segments` transaction/updated_at), SUR2-NEW-06 (mac-check-dylibs scope + whitespace splitting), SUR2-NEW-07 (spike texture-cache leak on surface error).

## Rules per fix

- Verify the finding in source first; if a finding is WRONG or does not apply, mark it WONTFIX with the reason in the report rather than changing code to fit.
- The smallest correct fix. No new dependencies. No behavior redesigns (e.g. MED-02's `_exit` is a deliberate ggml-teardown workaround — fix the *checkpoint/destructor* gap around it, not the design; read the report's note).
- For SUR2-NEW-04: the quit path (`LiveDecoder` token) must be cancellable so teardown does not wait on an inference window — that was audit 1's remediation too. Keep the change to the token plumbing.
- For SUR-MED-05 (settings.toml): write temp + rename (atomic) in the same dir; fsync optional, note the choice.
- For SUR-MED-01/zip: cap total uncompressed bytes (a sane, documented constant, e.g. 5 GB across the archive); `Read::take` per entry is fine; temp-sibling collisions get a unique suffix; prefix detection via common path prefix (note the archive set we actually extract — models zips — and pick the rule that cannot misroute those).
- Tests: every fix that can be unit-tested gets one (host-runnable). The resampler/overflow items need boundary tests (e.g. zero-length input, integer-extreme buffer sizes). Zip: a crafted-hostile-ish small zip test (deep nesting + oversize cap) without real bombs.
- `scripts/ask_glm.py` (MED-06) is tooling, not shipped code — still fix it (tempfile module, no fixed predictable path).

## Do NOT

- No git commit/push; no sudo; only files inside `/home/ubuntu/suramak`; no new dependencies; no touches to `.github/workflows/ci.yml`, `Cargo.toml` workspace deps, or the android Gradle build (the APK already shipped).

## Verification + report

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` (511 baseline), `cargo test -p suramak-asr --features capture,capture-test`.
- Windows-target C-free checks if you touch anything cfg-gated Windows (`suramak-glass`/`suramak-taps` targets check; use the probe pattern from `docs/reports/M14b.md` §2.2 for windows-only UI sources).
- Write `docs/reports/audit-batch-3.md`: per finding — verdict (FIXED / WONTFIX + why), the change, the test added. Close with the leftover count (expect 0 unresolved).
