# In-app updates (macOS)

In-app updates are **macOS-only** and live behind the default-OFF `updates`
cargo feature; the updater is compiled out of every other build. This page is
the developer-facing detail that used to sit in the README.

## What the packaged app does

Sparkle 2 checks the feed
(`https://github.com/LoLyeah/suramak/releases/latest/download/appcast.xml`)
in the background — roughly once a day, controlled by Settings → **Updates →
"Automatic updates"** (ON by default) — and, when a newer version exists,
shows its standard dialog with the release notes (the CHANGELOG section of
that release) and **installs in place only after you confirm**. The app menu
also gets a **"Check for Updates…"** item (greyed out while a check runs). The
app log (`RUST_LOG=suramak=debug`) records every step.

## Build and release wiring

```bash
./scripts/fetch-sparkle.sh          # pinned + sha256-verified Sparkle 2.9.6 (framework + tools)
SPARKLE_FRAMEWORK_PATH="$(./scripts/fetch-sparkle.sh)" \
  cargo build --release -p suramak-ui --features updates   # dev build (needs the framework on disk)
SURAMAK_FEATURES=speech-transcriber,updates ./scripts/make-app.sh   # the release bundle (embeds + signs Sparkle.framework)
```

- `scripts/fetch-sparkle.sh` pins the official 2.9.6 archive and its real
  sha256; the release and CI pipelines never download the framework any other way.
- `make-app.sh` copies the framework into `Contents/Frameworks` (symlinks
  preserved) before the existing ad-hoc signing; the
  `@executable_path/../Frameworks` rpath is a link-time argument
  (`crates/suramak-ui/build.rs`).
- The release workflow signs the archive and writes `appcast.xml` with
  Sparkle's `generate_appcast` (`scripts/make-appcast.sh`), using the
  `SPARKLE_ED_KEY` repo secret via `--ed-key-file -` (stdin, the documented CI
  path). Without that secret the release still ships and the appcast step logs
  a loud skip — the updater then finds no feed and stays dormant.

## One-time key setup (no key material ever enters the repo)

1. On a Mac, from the fetched distribution (`build/sparkle/2.9.6`):
   `./bin/generate_keys` — generates the EdDSA keypair into your login
   keychain and prints the public key.
2. Put the printed public key into `packaging/Info.plist` as `SUPublicEDKey`
   (replacing the clearly-marked placeholder).
3. Export the private key and store it as the repo secret — the exported file
   *is* the base64 key string (`generate_keys -x` writes it in exactly the form
   `--ed-key-file -` expects), so its content goes in as-is:
   `./bin/generate_keys -x /tmp/sparkle_ed_key && pbcopy < /tmp/sparkle_ed_key`
   → GitHub → repo → Settings → Secrets and variables → Actions → new secret
   **`SPARKLE_ED_KEY`** → paste. Delete the local file afterwards. (Do **not**
   base64 the file again — a double-encoded value is the one common mistake,
   and the appcast step refuses it with a message saying exactly this.)

Exact commands, the expected first-cycle behaviour and the honest boundaries
are in `docs/reports/M12.md`; the owner smoke is `docs/USER_SMOKE.md` §32.
