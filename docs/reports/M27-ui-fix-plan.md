# M27 — UI/UX fix plan (from the owner's phone screenshots)

Owner report, 07:52–07:53 on the Vivo: a job ran and finished (notification:
"Transcript ready · suramak-test-en-2min · 13 segments") while the Library
behind it still said "No transcripts yet"; the Editor's bottom bar painted the
timecode through "Tag speakers"; segment headers carried `Line length 165`; and
one state painted word chips at body size across three lines, over the rows
below.

Plan produced by `gemini-4-neon` (high effort) against those screenshots, then
verified against the code and trimmed to what is actually broken.

## Defects, ranked

| # | Defect | Class | Status |
|---|---|---|---|
| 1 | No in-app job progress; the Library is blank while a job runs | state + event plumbing | **fixed** (`f4d518e`… strip fed from the host) |
| 2 | Library does not refresh when a job finishes | state + event plumbing | **fixed** (`0514c1c`: the app wakes once a second; the store-vs-shown poll then reloads) |
| 3 | Bottom bar: timecode paints through "Tag speakers" | layout arithmetic (fixed x offsets) | **fixed** (`f4d518e`: allocated from the right edge inward, measured widths, timecode clipped + ellipsis) |
| 4 | `Line length 165` / `Pace 21.3` in the reading surface | presentation (subtitle-editor metric) | **fixed** (`f4d518e`: the advisory is gone from the phone) |
| 5 | Two Save/Cancel pairs at once | state | **not a defect** — `edit_buffer` is a single `Option`, so one row edits at a time; the two screenshots were two scroll positions |
| 6 | Word chips wrap over the rows below | layout arithmetic (unbounded wrap, fixed reserved height) | **fixed** (`f4d518e`: one clipped line that follows the spoken word, `…` for the rest) |

## The in-app job surface (Neon's design, adopted)

The Library already had the surface — `JobView`, `CompactApp::on_job`,
`job_finished`, a progress strip with an eased fill — and **nothing on Android
ever fed it**. That is why progress lived only in the notification.

Now the shell feeds it every frame from the job host's own status:

- **copying** → `Preparing the file · 40 %`
- **decoding** → `Preparing the audio · 0 %`
- **transcribing** → `Transcribing · 42 %`
- no job → the strip disappears

The strip sits above the list and pushes it down; while it is visible the empty
state "No transcripts yet" is not painted, because a job in flight is not an
empty library. The label is the phase word, not the filename: the filename is in
the notification and in the resulting row.

## What Neon proposed that is deliberately not done yet

- **A cross-thread `JobEvent` channel** into `suramak-ui`. The frame poll reads
  the host's status instead; it is one mutex read per frame and cannot disagree
  with the notification, because both read the same struct. A channel would be
  faster only at 60 Hz with many jobs.
- **Editor banner for the running job.** The Editor is where a user reads; the
  strip is a Library affordance until the Editor has a job row it can carry
  without stealing the reading column.
- **Retry / Dismiss on a failed strip.** Failures are on the Diagnostics
  "Last job" card and in the notification's own Retry action.
- **Word-level inline highlighting** (a three-span `LayoutJob` on the active
  word). Nice, not needed: the strip already marks the spoken word.

## Acceptance criteria now in the suite

- The bottom bar's timecode, speakers control and Export button have disjoint
  rects at the phone viewport; the timecode ellipsises rather than colliding.
- No painted text or a11y label on a segment row contains `Line length`, `Pace`
  or `CPL`, selected or not.
- The word strip paints inside its own row (clipped), one line only.
- The Library does not paint the empty state while a job is in flight.

## Open questions for the owner

- On a `failed` job the strip disappears and the sentence lives in Diagnostics
  and the notification. Would you rather the Library keep a red strip with the
  reason until dismissed?
