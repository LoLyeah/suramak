# WORKORDER — M24A: the watched inbox (a folder in, transcripts out)

Roadmap: `docs/reports/M21-roadmap.md` #6 (Carbon high, from MacWhisper's
watched folders — the local-first version: no cloud, no account). Planned
release: v0.6.

## The user job

A recorder, a phone sync folder, a colleague's dropbox: point Suramak at a
folder once; every audio file that finishes landing there is imported and
queued automatically, and the Library says what it picked up. No dialogs, no
re-imports, no half-written files.

## Deliverables

1. **Watch settings** (additive): enabled flag + folder path, persisted in
   settings; desktop picks a folder with the existing picker; Android uses
   `ACTION_OPEN_DOCUMENT_TREE` with persistable permission and stores the tree
   URI (content resolver, not a raw path).
2. **A polling scanner** (no new crates): one background thread, every 5 s,
   bounded (≤ 200 entries per scan), depth ≤ 3. A file is eligible only when
   (a) its name has an audio extension, (b) its size is unchanged across two
   scans ≥ 2 s apart, (c) no `.part`/`.crdownload`/`.tmp` sibling and no
   in-progress marker in the name. Eligible files are deduped by **content
   hash** against a new `watched_files` table (path, size, mtime, sha256,
   imported_at, transcript_id); a re-scan imports nothing.
3. **The import path is the existing one**: a watched file becomes a normal
   queue job (same model choice, same language default, same dedupe on
   transcript source). Never a second transcription path.
4. **Honesty in the UI**: a Library status row when watching is on —
   "Watching · 3 imports" plus the folder's display name; a named failure when
   the folder disappears or permission is revoked ("the folder is no longer
   reachable"), never silence. New imports appear in the list as they land.
5. **Battery/IO discipline**: scanning pauses while the app is in the
   background on Android beyond a bounded window (a 5 s poll must not keep a
   phone awake); document the rule in the report.
6. **A test seam** so hosts can drive the scanner deterministically: the
   scanner takes its clock + `is_settled` predicate as parameters; tests use a
   fake tree and a fake clock.

## Verification

- Tests: settle gate rejects a growing file and accepts it once stable; a
  `.part` sibling blocks; dedupe by sha256 means the same bytes copied under a
  new name import once; a deleted folder surfaces the named failure; the scan
  is bounded (201 entries → 200 examined, logged).
- The Android SAF traversal gets a host-testable abstraction (a trait over
  "list entries + open a stream") with a fake implementation; the real SAF
  binding is compiled by the NDK gate only.
- fmt/clippy -D warnings; NDK check + clippy on both Android crates; one full
  workspace run (baseline 1342).
- Report: `docs/reports/M24a-watched-inbox.md` with the poll/IO numbers.

## Do NOT

- No new crates (polling, not inotify); no cloud folder APIs; no new
  transcription path; no changes to the queue's journal or resume; no
  Library/editor restyle beyond the one status row; no CI edits; no git
  commit/push; no sudo.
