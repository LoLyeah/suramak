# WORKORDER — M18.1: the manual speaker directory

Plan source: `docs/PLAN-M15-M18.md` §M18.1 (M-size). Builds on M16I's diarization
(schema v7: `diarization_runs` / `speakers` / `speaker_turns`, rename-in-place).
Contract from the plan: **user-created identities and explicit links from
session-local speakers. No automatic identity inference. Voice-embedding
storage stays outside this approval** — this slice stores names a human typed,
never a voiceprint, and never links a speaker across transcripts by itself.

## Deliverables

1. **Schema v8 (additive):** a `people` table (id, display name, created at,
   optional note) and a nullable `speakers.person_id` link. Upgrade-in-place
   from v7; corrupt/newer refusals unchanged. Deleting a person unlinks their
   speakers (their names stay as typed labels? No — see 3).
2. **The link is explicit only:** in the editor, a speaker badge gains
   "Link to person…" (pick an existing person or create one from the typed
   name). No suggestions, no fuzzy matching, no cross-transcript inference;
   the UI copy says so once ("You decide who this is — Suramak never guesses").
3. **Directory screen:** list people (name, count of linked speakers across
   transcripts, rename, delete). Delete = unlink everywhere and remove the
   person; the affected speaker labels revert to their pre-link label
   ("Speaker N"), never to an invented name.
4. **Naming rules:** renaming a person updates every linked speaker's displayed
   label; renaming a speaker directly (M16I's rename-in-place) unlinks it (an
   explicit override wins). Names are trimmed, non-empty, case-insensitively
   unique among people (refuse with a named inline error, never silently
   merge).
5. **A11y + motion:** labels/rings as M16I; no idle redraws; the phone inherits
   the shared Deck components.

## Verification

- Tests: v7→v8 upgrade from populated fixtures (nothing lost); link/unlink
  round-trips; person rename propagates to linked speakers only; speaker rename
  unlinks; delete-unlinks-all; uniqueness refusal; the no-inference guarantee
  (a speaker with no link never resolves to a person).
- Desktop golden shots: the badge menu with "Link to person…", the directory
  screen. fmt/clippy -D warnings; workspace tests (baseline 1133) green with
  ONE full run at the end; M15/M16I suites stay green.
- Report: `docs/reports/M18a-speaker-directory.md`.

## Do NOT

- No voice embeddings, no automatic matching, no cross-transcript speaker
  suggestions, no schema fields for biometrics; no engine changes; no Android
  specific code; no CI edits; no new deps; no git commit/push; no sudo.
