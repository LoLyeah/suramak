# WORKORDER-M4.5 — UI follow-ups: custom language codes + transcript delete

Base: post-M4 master (all crashes of 2026-10-13 fixed; app verified stable on the
owner's Mac). Two small, independent UI features. Keep every existing test green.

## F1 — Custom language codes (settings)

Current state: `crates/suramak-ui/src/app/settings_form.rs` defines the selector
set `LANGUAGES = ["auto", "en", "id"]`; `SettingsForm.language` is already a
`String` ("or a custom code from an older file" is even in its doc comment), so
the data model needs **no change** — the selector UI just needs to stop
constraining input.

Do:
- Settings ("Default language") offers the three presets plus **"Custom…"**;
  choosing Custom reveals a small text field for the code.
- Validation is loose and friendly: trim; accept 2–8 chars of ASCII letters/
  digits/`-` (whisper language codes are lowercase ISO-639-1-ish; do not be
  stricter); show an inline hint when invalid and refuse to persist that value
  (keep the previous selection). Strip to lowercase.
- The stored value stays what the user typed (e.g. `es`, `jv`); the worker
  plumbing already passes `settings.language` through — do not change it.
- Small caption under the field: unsupported codes are the user's own risk
  ("not officially supported — the model may produce poor results").
- Tests live in `settings_form.rs` (egui-free): custom code accepted + lowered,
  invalid input rejected with the previous value kept, presets still work.

## F2 — Trash button (delete a transcript)

`suramak_core::store::Store::delete_transcript(id)` **already exists** (schema
has `ON DELETE CASCADE` on segments) — this is UI + effect wiring only.

Do:
- New effect `DeleteTranscript { id: Uuid }` in `crates/suramak-ui/src/app/mod.rs`
  (next to `Effect::DeleteModel`; mirror its doc style). Emit it from the UI
  (see the `DeleteModel` emit + test pattern at `mod.rs:611` / `:1743` — add the
  same kind of test for delete).
- Runtime arm in `crates/suramak-ui/src/runtime.rs` (mirror the `DeleteModel`
  arm at ~line 221): call `store.delete_transcript`, then reload the library
  list; if the deleted transcript is currently open in the editor, navigate
  Home and set a short status ("deleted <title>"). Errors surface through the
  existing status mechanism, never a panic.
- UI: a discreet trash affordance in the **editor header** (`app/editor.rs`),
  with an inline confirm step (e.g. trash → "Delete? ✓ ✕" in place — follow the
  house confirm pattern used elsewhere, no OS dialog). After delete the sidebar
  row must disappear.
- Store test: if `delete_transcript` has no test proving rows+segments vanish
  and a random other transcript survives, add one in `store.rs`.

## Docs
- `docs/USER_SMOKE.md`: two new checklist items (custom code: Settings → Custom
  → `es` → survives restart; delete: trash + confirm → row gone → still gone
  after relaunch).
- `README.md`: extend the feature bullets minimally (custom language codes;
  delete from library).

## Acceptance (run from /home/ubuntu/suramak; report exact outputs)
1. `cargo fmt --check` — clean.
2. `cargo clippy --workspace --all-targets -- -D warnings` — clean (Linux).
3. `cargo test --workspace` — all green, plus the new tests.
4. `cargo build -p suramak-ui && SURAMAK_SMOKE_SECS=3 xvfb-run -a -s "-screen 0 1024x768x24" ./target/debug/suramak` — smoke ok (frames > 0).
5. Existing settings round-trip tests still pass (custom code persists via toml).

## Constraints
- Only modify files inside /home/ubuntu/suramak. No sudo. No git commit/push.
- Do not touch `suramak-glass`, `suramak-taps`, the winit fork, or scripts/.
- Do not change on-disk formats; settings.toml stays compatible.
- Write `docs/reports/M4.5.md` (what changed, test evidence, deviations).
