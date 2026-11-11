//! The compact UI's Android host (M13g1).
//!
//! This is the seam between the real on-device state and the shared Deck
//! screens in `suramak_ui::deck::compact`: it owns the library store, the
//! settings file and the `CompactApp` state machine, and answers one JSON
//! command per call (the same one-function bridge pattern as M13d/M13e/M13f).
//!
//! **What is host-testable** (and tested in `tests/compact_host.rs`): opening a
//! transcript from the completion notification lands on Library → Editor with
//! the row loaded; a deleted row lands on Library with the honest line; back
//! pops then finishes; a saved segment reaches sqlite; a setting reaches
//! `settings.toml`; the model table follows the M13D device tiers. **What is
//! not** (owner-device): drawing the surface with wgpu, touch/fling delivery,
//! and the platform back gesture itself — those are the M13b v2.4 shell's and
//! G2's job.
//!
//! The visible egui surface still belongs to the M13b spike in this G1 cut; the
//! host is what the notification and the future compact shell both call, so the
//! route arguments flow through one place instead of a Kotlin toast.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{json, Value};
use suramak_core::device::DeviceProfile;
use suramak_core::settings::{LangMode, Settings};
use suramak_core::store::Store;
use suramak_ui::deck::compact::{
    CompactAction, CompactApp, CompactExportFormat, CompactRoute, CompactSetting, CompactSettings,
    Sheet,
};

use crate::export_job::ExportJobs;
use crate::jobs::HostDirs;

/// One host per process (the JNI `init` creates it).
pub struct CompactHost {
    inner: Mutex<Inner>,
}

struct Inner {
    app: CompactApp,
    dirs: HostDirs,
    settings_path: PathBuf,
    /// The last device probe; the download card and `download_state` reuse it.
    profile: Option<DeviceProfile>,
    /// The export job slot (single-flight; staging under `files/exports`).
    exports: ExportJobs,
}

impl CompactHost {
    /// Opens the library and settings under the app's files directory.
    pub fn new(root: &Path) -> Self {
        let dirs = HostDirs::under(root);
        let settings_path = root.join("settings.toml");
        let settings = Settings::load(&settings_path).unwrap_or_default();
        let compact = compact_settings(&settings);
        let mut app = CompactApp::new(compact);
        let language = match &settings.asr.language {
            LangMode::Auto => "auto".to_string(),
            LangMode::Fixed(lang) => lang.clone(),
        };
        app.settings.language = language;
        app.first_run_seen = settings.ui.first_run_seen;
        let mut inner = Inner {
            app,
            dirs,
            settings_path,
            profile: None,
            exports: ExportJobs::new(root),
        };
        inner.reload_library();
        Self {
            inner: Mutex::new(inner),
        }
    }

    /// Runs `f` against the one app state (the Android shell draws it directly;
    /// the JNI ops mutate the same object, so the phone never has two routes).
    pub fn with_app<R>(&self, f: impl FnOnce(&mut CompactApp) -> R) -> Option<R> {
        self.inner.lock().ok().map(|mut inner| f(&mut inner.app))
    }

    /// Runs one JSON command. Never panics on bad input.
    pub fn dispatch(&self, request: &str) -> String {
        let Ok(value) = serde_json::from_str::<Value>(request) else {
            return error("invalid JSON request");
        };
        let op = value.get("op").and_then(Value::as_str).unwrap_or_default();
        let Ok(mut inner) = self.inner.lock() else {
            return error("the UI state lock is poisoned");
        };
        match op {
            "library" => {
                inner.reload_library();
                inner.snapshot()
            }
            "route" => inner.snapshot(),
            "open_transcript" => {
                let id = value
                    .get("transcript_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                inner.open_transcript(id)
            }
            "back" => inner.back(),
            "save_segment" => inner.save_segment(&value),
            "setting" => inner.setting(&value),
            "device" => inner.device(&value),
            "download_state" => inner.download_state(),
            "refresh_models" => inner.refresh_models(),
            "export" => inner.prepare_export(&value, false),
            "export_share" => inner.prepare_export(&value, true),
            "export_done" => inner.finish_export(&value),
            "export_shared" => inner.finish_shared(&value),
            "export_cancel" => inner.cancel_export(&value),
            "export_failed" => inner.fail_export(&value),
            "quit" => inner.quit(),
            "version" => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }).to_string(),
            other => error(&format!("unknown op '{other}'")),
        }
    }
}

impl Inner {
    fn reload_library(&mut self) {
        match Store::open(&self.dirs.db_path) {
            Ok(store) => match store.list_transcripts() {
                Ok(items) => self.app.set_library(items),
                Err(err) => self.app.set_library_error(err.to_string()),
            },
            Err(err) => self.app.set_library_error(err.to_string()),
        }
    }

    /// The route/state payload the Kotlin side mirrors (and the host tests
    /// assert). Deliberately small: screen, depth, the open transcript, the
    /// sheet and any status line.
    fn snapshot(&self) -> String {
        let route = match self.app.nav.current() {
            CompactRoute::Library => "library",
            CompactRoute::Editor => "editor",
            CompactRoute::Settings => "settings",
            CompactRoute::Diagnostics => "diagnostics",
        };
        json!({
            "ok": true,
            "screen": route,
            "depth": self.app.nav.depth(),
            "transcript_id": self.app.editor.as_ref().map(|editor| editor.transcript.id.to_string()),
            "title": self.app.editor.as_ref().map(|editor| editor.transcript.title.clone()),
            "sheet": match self.app.sheet { Some(Sheet::Export) => "export", Some(Sheet::Quit) => "quit", Some(Sheet::Onboarding) => "onboarding", None => "none" },
            "status": self.app.status.as_ref().map(|(text, error)| json!({"text": text, "error": error})),
            "library_rows": self.app.rows.len(),
            "first_run_seen": self.app.first_run_seen,
            "recording": self.app.recording,
            "models": self.app.models.iter().map(|row| json!({
                "id": row.id,
                "offered": row.offered,
                "installed": row.installed,
                "recommended": row.recommended,
            })).collect::<Vec<_>>(),
            "library_error": self.app.library_error.clone(),
            "export": self.app.export_session.as_ref().map(|session| json!({
                "transcript": session.transcript.to_string(),
                "format": session.format.extension(),
                "bilingual": session.bilingual,
                "phase": match session.phase {
                    suramak_ui::deck::compact::ExportPhase::Choosing => "choosing",
                    suramak_ui::deck::compact::ExportPhase::Writing => "writing",
                    suramak_ui::deck::compact::ExportPhase::Written => "written",
                    suramak_ui::deck::compact::ExportPhase::Shared => "shared",
                    suramak_ui::deck::compact::ExportPhase::Failed => "failed",
                },
                "name": session.name,
                "bytes": session.bytes,
                "error": session.error,
            })),
        })
        .to_string()
    }

    fn open_transcript(&mut self, id: &str) -> String {
        let now = self.started_now();
        let Ok(uuid) = uuid::Uuid::parse_str(id) else {
            self.app.deep_link_missing(uuid::Uuid::nil(), now);
            return self.snapshot_with_error("that transcript id is not valid");
        };
        let found = Store::open(&self.dirs.db_path)
            .ok()
            .and_then(|store| store.get_transcript(uuid).ok());
        match found {
            Some(transcript) => {
                self.reload_library();
                let store = Store::open(&self.dirs.db_path).ok();
                let translations = store
                    .as_ref()
                    .and_then(|store| store.segment_translations(uuid).ok())
                    .unwrap_or_default();
                let words = store
                    .as_ref()
                    .and_then(|store| store.word_alignment(uuid).ok())
                    .unwrap_or_default();
                self.app.deep_link_words(transcript, words, now);
                if let Some(editor) = self.app.editor.as_mut() {
                    editor.has_translation = !translations.is_empty();
                }
                self.snapshot()
            }
            None => {
                self.app.deep_link_missing(uuid, now);
                self.snapshot()
            }
        }
    }

    fn back(&mut self) -> String {
        let outcome = self.app.back(self.started_now());
        // Back out of an export sheet also releases the staged job: the sheet
        // is gone, so nothing can ever finish it and a second export would be
        // refused as "already in flight".
        if outcome == suramak_ui::deck::nav::BackOutcome::Dismissed {
            if let Some(live) = self.exports.live() {
                self.exports.cancel(&live.job);
            }
        }
        // M13g3: a back gesture on the one-time intro is Skip; persist it here
        // because the gesture has no action vector of its own.
        if self.app.take_onboarding_seen() {
            self.persist_first_run_seen(true);
        }
        self.app.nav.settle();
        let mut reply: Value =
            serde_json::from_str(&self.snapshot()).unwrap_or_else(|_| json!({ "ok": true }));
        reply["back"] = json!(match outcome {
            suramak_ui::deck::nav::BackOutcome::Popped => "popped",
            suramak_ui::deck::nav::BackOutcome::Dismissed => "dismissed",
            suramak_ui::deck::nav::BackOutcome::Finish => "finish",
        });
        reply["finish"] = json!(outcome == suramak_ui::deck::nav::BackOutcome::Finish);
        reply.to_string()
    }

    fn save_segment(&mut self, value: &Value) -> String {
        let idx = value.get("idx").and_then(Value::as_u64).unwrap_or(0) as u32;
        let text = value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(editor) = self.app.editor.as_mut() else {
            return error("no transcript is open");
        };
        let id = editor.transcript.id;
        editor.edit(idx, text.to_string());
        let edits = editor.pending_edits();
        let store = match Store::open(&self.dirs.db_path) {
            Ok(store) => store,
            Err(err) => {
                self.app.on_save_failed(err.to_string());
                return error(&err.to_string());
            }
        };
        for edit in &edits {
            if let Err(err) = store.update_segment_text(id, edit.idx, &edit.text) {
                self.app.on_save_failed(err.to_string());
                return error(&err.to_string());
            }
        }
        self.app.on_saved();
        self.snapshot()
    }

    fn setting(&mut self, value: &Value) -> String {
        let Some(kind) = value.get("setting").and_then(Value::as_str) else {
            return error("setting needs a 'setting' name");
        };
        let mut settings = Settings::load(&self.settings_path).unwrap_or_default();
        match kind {
            "model" => {
                let id = value
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if id.is_empty() {
                    return error("model needs a value");
                }
                settings.asr.model_id = id.clone();
                self.app.apply_setting(CompactSetting::Model(id));
            }
            "language" => {
                let lang = value.get("value").and_then(Value::as_str).unwrap_or("auto");
                settings.asr.language = if lang == "auto" {
                    LangMode::Auto
                } else {
                    LangMode::Fixed(lang.to_string())
                };
                self.app
                    .apply_setting(CompactSetting::Language(lang.to_string()));
            }
            "appearance" => {
                let appearance = match value.get("value").and_then(Value::as_str) {
                    Some("light") => suramak_core::settings::Appearance::Light,
                    Some("dark") => suramak_core::settings::Appearance::Dark,
                    _ => suramak_core::settings::Appearance::System,
                };
                settings.ui.appearance = appearance;
                self.app
                    .apply_setting(CompactSetting::Appearance(appearance));
            }
            "keep_audio" => {
                let keep = value.get("value").and_then(Value::as_bool).unwrap_or(false);
                settings.storage.keep_audio = keep;
                self.app.apply_setting(CompactSetting::KeepAudio(keep));
            }
            "first_run_seen" => {
                let seen = value.get("value").and_then(Value::as_bool).unwrap_or(true);
                settings.ui.first_run_seen = seen;
                self.app.apply_setting(CompactSetting::FirstRunSeen(seen));
            }
            other => return error(&format!("unknown setting '{other}'")),
        }
        if let Err(err) = settings.save(&self.settings_path) {
            return error(&err.to_string());
        }
        self.snapshot()
    }

    fn device(&mut self, value: &Value) -> String {
        let number = |name: &str| value.get(name).and_then(Value::as_u64).unwrap_or(0);
        let profile = DeviceProfile::new(
            number("total_ram_bytes"),
            number("available_ram_bytes"),
            value
                .get("low_memory")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            value.get("arm64").and_then(Value::as_bool).unwrap_or(true),
        );
        let installed = installed_models(&self.dirs.models_dir);
        self.profile = Some(profile);
        self.app.seed_device(profile, &installed);
        self.app
            .set_free_space(value.get("free_bytes").and_then(Value::as_u64));
        if let Some(reduce) = value.get("reduce_motion").and_then(Value::as_bool) {
            self.app.reduce_motion = reduce;
        }
        self.snapshot()
    }

    /// Rescans `files/models` after a job finishes, adopts the new model as the
    /// setting when nothing else is installed, and re-derives the card.
    fn refresh_models(&mut self) -> String {
        let installed = installed_models(&self.dirs.models_dir);
        self.app.refresh_models(&installed);
        if let Some(profile) = self.profile {
            if self.app.model_card_needed() {
                self.app.seed_device(profile, &installed);
            }
        }
        if self
            .app
            .download
            .as_ref()
            .is_some_and(|download| installed.iter().any(|id| id == &download.model))
        {
            if let Some(model) = self.app.recommended_model().map(|row| row.id.clone()) {
                if installed.iter().any(|id| id == &model) {
                    self.app.apply_setting(CompactSetting::Model(model.clone()));
                    let mut settings = Settings::load(&self.settings_path).unwrap_or_default();
                    settings.asr.model_id = model;
                    let _ = settings.save(&self.settings_path);
                }
            }
            self.app.clear_download();
        }
        self.snapshot()
    }

    /// The `.part` a killed run left, at the job's own percent. The card offers
    /// **Resume** from this; the next `start` appends from the same byte.
    fn download_state(&mut self) -> String {
        let Some(profile) = self.profile else {
            return json!({ "ok": true, "known": false }).to_string();
        };
        let recommendation = suramak_core::device::recommend(profile, &self.app.settings.language);
        let Some(model_id) = recommendation.default_model_id else {
            return json!({ "ok": true, "known": false }).to_string();
        };
        let installed = installed_models(&self.dirs.models_dir);
        if installed.iter().any(|id| id == model_id) {
            return json!({ "ok": true, "known": true, "model": model_id, "installed": true })
                .to_string();
        }
        let Some(info) = suramak_core::models::find_model(model_id) else {
            return error("the recommended model is not in the manifest");
        };
        let part = suramak_core::download::part_path(&self.dirs.models_dir.join(info.hf_file));
        let bytes_done = std::fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0);
        if bytes_done == 0 {
            return json!({ "ok": true, "known": true, "model": model_id, "installed": false })
                .to_string();
        }
        let percent = crate::model_host::percent(bytes_done, info.size_bytes);
        json!({
            "ok": true,
            "known": true,
            "model": model_id,
            "installed": false,
            "bytes_done": bytes_done,
            "bytes_total": info.size_bytes,
            "percent": percent,
        })
        .to_string()
    }

    /// Writes `first_run_seen` without touching the other settings.
    fn persist_first_run_seen(&mut self, seen: bool) {
        let mut settings = Settings::load(&self.settings_path).unwrap_or_default();
        settings.ui.first_run_seen = seen;
        if let Err(err) = settings.save(&self.settings_path) {
            self.app
                .on_save_failed(format!("could not save the intro state: {err}"));
        }
    }

    /// `export` / `export_share`: render through the M15 serializer, stage the
    /// bytes atomically, and hand Kotlin the one file. The bytes exist before
    /// the platform stream is opened; `export_done` verifies the write.
    fn prepare_export(&mut self, value: &Value, share: bool) -> String {
        let format = match value.get("format").and_then(Value::as_str) {
            Some("txt") => CompactExportFormat::Txt,
            Some("json") => CompactExportFormat::Json,
            Some("srt") => CompactExportFormat::Srt,
            Some("vtt") => CompactExportFormat::Vtt,
            Some("tsv") => CompactExportFormat::Tsv,
            _ => CompactExportFormat::Md,
        };
        let bilingual = value
            .get("bilingual")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let Some(editor) = self.app.editor.as_mut() else {
            return error("no transcript is open");
        };
        editor.format = format;
        let id = editor.transcript.id;
        let suggested_name = editor.suggested_name();
        // The library list is a summary (no segments); the export reads the
        // full row, exactly as the editor's open path does.
        let Some(transcript) = Store::open(&self.dirs.db_path)
            .ok()
            .and_then(|store| store.get_transcript(id).ok())
        else {
            return error("the transcript is no longer in the library");
        };
        let translations = if bilingual {
            Store::open(&self.dirs.db_path)
                .ok()
                .and_then(|store| store.segment_translations(id).ok())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let now = self.started_now();
        // The UI's Export/Share button normally creates the session before the
        // action reaches us; a host-only dispatch (process restart, tests) must
        // create the same state instead of leaving the sheet blank.
        self.app.export_bilingual = bilingual;
        self.app.open_export_sheet(now);
        let _ = self.app.begin_export(now);
        self.app.on_export_picked();
        match self.exports.stage(
            &transcript,
            &translations,
            format,
            bilingual,
            &suggested_name,
        ) {
            Ok(staged) => json!({
                "ok": true,
                "job": staged.job,
                "path": staged.path.to_string_lossy(),
                "suggested_name": staged.name,
                "bytes": staged.bytes,
                "checksum": staged.checksum,
                "mime": staged.mime,
                "bilingual": staged.bilingual,
                "share": share,
                "screen": "editor",
                "sheet": "export",
            })
            .to_string(),
            Err(err) => {
                self.app.on_export_failed(err.clone());
                error(&err)
            }
        }
    }

    /// The picker's stream is closed: verify the byte count, clean the staging
    /// directory, and report the saved file to the sheet.
    fn finish_export(&mut self, value: &Value) -> String {
        let job = value.get("job").and_then(Value::as_str).unwrap_or_default();
        let bytes = value.get("bytes").and_then(Value::as_u64).unwrap_or(0);
        match self.exports.finish(job, bytes) {
            Ok(staged) => {
                self.app.on_export_written(staged.name, staged.bytes);
                self.snapshot()
            }
            Err(err) => {
                self.app.on_export_failed(err.clone());
                error(&err)
            }
        }
    }

    /// `ACTION_SEND` got its URI: the staged file stays for the receiving app
    /// and is cleaned by the next process start.
    fn finish_shared(&mut self, value: &Value) -> String {
        let job = value.get("job").and_then(Value::as_str).unwrap_or_default();
        match self.exports.release_for_share(job) {
            Ok(staged) => {
                self.app.on_export_shared(staged.name);
                self.snapshot()
            }
            Err(err) => {
                self.app.on_export_failed(err.clone());
                error(&err)
            }
        }
    }

    /// The destination write failed (a short write, a provider that rewrote the
    /// stream, a checksum mismatch). Release the staged job — the sheet's Try
    /// again re-stages — and report the host's own reason.
    fn fail_export(&mut self, value: &Value) -> String {
        if let Some(job) = value.get("job").and_then(Value::as_str) {
            self.exports.cancel(job);
        }
        let reason = value
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or("the destination could not be written");
        self.app.on_export_failed(reason);
        self.snapshot()
    }

    /// The picker/chooser was dismissed: the sheet returns to its chips and the
    /// staged bytes are dropped.
    fn cancel_export(&mut self, value: &Value) -> String {
        if let Some(job) = value.get("job").and_then(Value::as_str) {
            self.exports.cancel(job);
        }
        self.app.on_export_cancelled();
        self.snapshot()
    }

    fn quit(&mut self) -> String {
        let action = self.app.confirm_quit();
        debug_assert_eq!(action, CompactAction::Quit);
        json!({
            "ok": true,
            "steps": ["capture", "jobs", "services", "exit"],
            "note": "capture is stopped and handed off before the service and the process exit",
        })
        .to_string()
    }

    fn started_now(&self) -> f64 {
        // `Inner` does not own the clock; the caller's host does. Navigation only
        // needs a monotonic value, so the process uptime is enough.
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_secs_f64()
    }
}

impl Inner {
    fn snapshot_with_error(&self, reason: &str) -> String {
        let mut value: Value =
            serde_json::from_str(&self.snapshot()).unwrap_or_else(|_| json!({ "ok": true }));
        value["error"] = json!(reason);
        value.to_string()
    }
}

/// `Settings` → the compact form (the fields the phone edits).
fn compact_settings(settings: &Settings) -> CompactSettings {
    CompactSettings {
        model_id: settings.asr.model_id.clone(),
        language: match &settings.asr.language {
            LangMode::Auto => "auto".to_string(),
            LangMode::Fixed(lang) => lang.clone(),
        },
        appearance: settings.ui.appearance,
        keep_audio: settings.storage.keep_audio,
    }
}

/// Which manifest models have a file in `<root>/models`.
fn installed_models(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let files: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    suramak_core::models::MODELS
        .iter()
        .filter(|model| files.iter().any(|file| file == model.hf_file))
        .map(|model| model.id.to_string())
        .collect()
}

fn error(message: &str) -> String {
    json!({ "ok": false, "error": message }).to_string()
}

/// The process-wide host (created by `init` on the JNI side).
pub fn host() -> Option<&'static CompactHost> {
    HOST.get()
}

static HOST: std::sync::OnceLock<CompactHost> = std::sync::OnceLock::new();

/// Handles the protocol, including the one command that creates the host.
pub fn dispatch(request: &str) -> String {
    let value = match serde_json::from_str::<Value>(request) {
        Ok(value) => value,
        Err(_) => return error("invalid JSON request"),
    };
    let op = value.get("op").and_then(Value::as_str).unwrap_or_default();
    if op == "init" {
        let root = value
            .get("root")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if root.is_empty() {
            return error("init needs the app files directory");
        }
        #[cfg(target_os = "android")]
        suramak_core::android::set_data_dir(PathBuf::from(root));
        let host = HOST.get_or_init(|| CompactHost::new(Path::new(root)));
        // `init` creates the host and answers with its first library snapshot.
        // It must NOT be forwarded to `CompactHost::dispatch`, which has no
        // `init` arm — forwarding it returned `{"ok":false,"error":"unknown op
        // 'init'"}` to Kotlin (visible in logcat on the CI emulator), which
        // read as a failed initialisation even though the host existed.
        return host.dispatch(r#"{"op":"library"}"#);
    }
    match host() {
        Some(host) => host.dispatch(request),
        // Back on a surface whose host is not up yet is a LEAVE: the splash
        // has no navigation stack to pop, and swallowing the gesture traps
        // the user on "Opening the library…" (observed on a slow emulator —
        // and the Activity never finished, so the finish-path exit never
        // ran). The reply must carry `finish:true` in the shape the real
        // `back` op uses.
        None if op == "back" => json!({
            "ok": true,
            "back": "finish",
            "finish": true,
        })
        .to_string(),
        None => error("the compact UI host has not been initialised (send init first)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suramak_core::settings::Appearance;
    use suramak_core::transcript::{Segment, Source, Transcript};

    fn fixture(dir: &Path) -> (Store, uuid::Uuid) {
        std::fs::create_dir_all(dir).expect("dir");
        let store = Store::open(&dir.join("suramak.db")).expect("store");
        let transcript = Transcript::new(
            "Field notes".to_string(),
            Source::File {
                name: "notes.m4a".to_string(),
            },
            "base".to_string(),
            Some("en".to_string()),
            None,
            vec![
                Segment {
                    idx: 0,
                    start_ms: 0,
                    end_ms: 3_000,
                    text: "first line".to_string(),
                },
                Segment {
                    idx: 1,
                    start_ms: 3_000,
                    end_ms: 6_000,
                    text: "second line".to_string(),
                },
            ],
        );
        store.insert_transcript(&transcript).expect("insert");
        (store, transcript.id)
    }

    #[test]
    fn a_notification_tap_opens_library_then_the_finished_editor() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        let (_, id) = fixture(&dir);
        let host = CompactHost::new(&dir);
        let reply: Value = serde_json::from_str(
            &host.dispatch(
                &json!({
                    "op": "open_transcript",
                    "transcript_id": id.to_string(),
                })
                .to_string(),
            ),
        )
        .expect("json");
        assert_eq!(reply["screen"], "editor");
        assert_eq!(reply["depth"], 2);
        assert_eq!(reply["title"], "Field notes");
        assert_eq!(reply["transcript_id"], id.to_string());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_deleted_transcript_lands_on_the_library_with_a_note() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        fixture(&dir);
        let host = CompactHost::new(&dir);
        let reply: Value = serde_json::from_str(
            &host.dispatch(
                &json!({
                    "op": "open_transcript",
                    "transcript_id": uuid::Uuid::new_v4().to_string(),
                })
                .to_string(),
            ),
        )
        .expect("json");
        assert_eq!(reply["screen"], "library");
        assert_eq!(reply["depth"], 1);
        assert!(reply["status"]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("no longer"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_saved_segment_reaches_sqlite() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        let (store, id) = fixture(&dir);
        let host = CompactHost::new(&dir);
        let _ = host.dispatch(
            &json!({
                "op": "open_transcript",
                "transcript_id": id.to_string(),
            })
            .to_string(),
        );
        let reply: Value = serde_json::from_str(
            &host.dispatch(
                &json!({
                    "op": "save_segment",
                    "idx": 0,
                    "text": "edited line",
                })
                .to_string(),
            ),
        )
        .expect("json");
        assert_eq!(reply["screen"], "editor");
        let stored = Store::open(&dir.join("suramak.db"))
            .expect("store")
            .get_transcript(id)
            .expect("row");
        assert_eq!(stored.segments[0].text, "edited line");
        let _ = store;
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn settings_round_trip_through_settings_toml() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        fixture(&dir);
        let host = CompactHost::new(&dir);
        let _ = host.dispatch(
            &json!({
                "op": "setting", "setting": "appearance", "value": "light"
            })
            .to_string(),
        );
        let _ = host.dispatch(
            &json!({
                "op": "setting", "setting": "language", "value": "id"
            })
            .to_string(),
        );
        let saved = Settings::load(&dir.join("settings.toml")).expect("settings");
        assert_eq!(saved.ui.appearance, Appearance::Light);
        assert_eq!(saved.asr.language, LangMode::Fixed("id".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_intro_flag_is_persisted_additively_and_a_new_host_reads_it() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        fixture(&dir);
        let host = CompactHost::new(&dir);
        let route: Value = serde_json::from_str(&host.dispatch(r#"{"op":"route"}"#)).expect("json");
        assert_eq!(
            route["first_run_seen"], false,
            "a fresh install shows the intro"
        );
        let saved: Value = serde_json::from_str(&host.dispatch(
            &json!({ "op": "setting", "setting": "first_run_seen", "value": true }).to_string(),
        ))
        .expect("json");
        assert_eq!(saved["first_run_seen"], true);
        let settings = Settings::load(&dir.join("settings.toml")).expect("settings");
        assert!(settings.ui.first_run_seen);
        // A relaunch constructs a new host over the same settings file.
        let relaunched = CompactHost::new(&dir);
        let route: Value =
            serde_json::from_str(&relaunched.dispatch(r#"{"op":"route"}"#)).expect("json");
        assert_eq!(route["first_run_seen"], true, "once only");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_state_reports_the_part_left_by_a_killed_run() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        fixture(&dir);
        let host = CompactHost::new(&dir);
        let _ = host.dispatch(
            &json!({
                "op": "device",
                "total_ram_bytes": 6_000_000_000u64,
                "available_ram_bytes": 2_000_000_000u64,
                "low_memory": false,
                "arm64": true,
                "free_bytes": 9_000_000_000u64,
            })
            .to_string(),
        );
        let base = suramak_core::models::find_model("base").expect("base");
        let part = suramak_core::download::part_path(&dir.join("models").join(base.hf_file));
        std::fs::create_dir_all(part.parent().expect("models dir")).expect("models dir");
        std::fs::write(&part, vec![0u8; 1_000]).expect("part");
        let reply: Value =
            serde_json::from_str(&host.dispatch(r#"{"op":"download_state"}"#)).expect("json");
        assert_eq!(reply["known"], true);
        assert_eq!(reply["model"], "base");
        assert_eq!(reply["bytes_done"], 1_000);
        assert_eq!(
            reply["percent"],
            crate::model_host::percent(1_000, base.size_bytes)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn back_pops_then_finishes_at_the_library_root() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        let (_, id) = fixture(&dir);
        let host = CompactHost::new(&dir);
        let _ = host.dispatch(
            &json!({
                "op": "open_transcript", "transcript_id": id.to_string()
            })
            .to_string(),
        );
        let popped: Value = serde_json::from_str(&host.dispatch(r#"{"op":"back"}"#)).expect("json");
        assert_eq!(popped["back"], "popped");
        assert_eq!(popped["screen"], "library");
        assert_eq!(popped["finish"], false);
        let finished: Value =
            serde_json::from_str(&host.dispatch(r#"{"op":"back"}"#)).expect("json");
        assert_eq!(finished["back"], "finish");
        assert_eq!(finished["finish"], true);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_export_stages_exactly_the_desktop_serializer_bytes() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        let (store, id) = fixture(&dir);
        let host = CompactHost::new(&dir);
        let _ = host.dispatch(
            &json!({ "op": "open_transcript", "transcript_id": id.to_string() }).to_string(),
        );
        let reply: Value = serde_json::from_str(
            &host.dispatch(&json!({ "op": "export", "format": "srt" }).to_string()),
        )
        .expect("json");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["mime"], "application/x-subrip");
        let path = reply["path"].as_str().expect("staged path");
        let staged = std::fs::read(path).expect("staged bytes");
        let transcript = store.get_transcript(id).expect("row");
        let expected = suramak_core::export::to_srt(&transcript).expect("render");
        assert_eq!(staged, expected.as_bytes(), "byte parity with the desktop");
        assert_eq!(reply["bytes"], staged.len() as u64);
        assert_eq!(
            reply["checksum"],
            crate::export_job::checksum_hex(crate::export_job::fnv1a64(&staged))
        );

        let done: Value = serde_json::from_str(
            &host.dispatch(
                &json!({
                    "op": "export_done",
                    "job": reply["job"],
                    "bytes": reply["bytes"],
                })
                .to_string(),
            ),
        )
        .expect("json");
        assert_eq!(done["export"]["phase"], "written");
        assert_eq!(done["status"]["text"], "Saved Field notes.srt");
        assert!(!std::path::Path::new(path).exists(), "staging is cleaned");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bilingual_export_stacks_the_stored_translation() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        let (store, id) = fixture(&dir);
        let records = vec![
            (
                0,
                suramak_core::translate::TranslationRecord::new(
                    "baris pertama",
                    "m",
                    "2026-01-01T00:00:00Z",
                    vec![0],
                ),
            ),
            (
                1,
                suramak_core::translate::TranslationRecord::new(
                    "baris kedua",
                    "m",
                    "2026-01-01T00:00:00Z",
                    vec![1],
                ),
            ),
        ];
        store.set_translations(id, &records).expect("translations");
        let host = CompactHost::new(&dir);
        let opened: Value = serde_json::from_str(&host.dispatch(
            &json!({ "op": "open_transcript", "transcript_id": id.to_string() }).to_string(),
        ))
        .expect("json");
        assert_eq!(opened["ok"], true, "the editor sees the translation");
        let reply: Value =
            serde_json::from_str(&host.dispatch(
                &json!({ "op": "export", "format": "srt", "bilingual": true }).to_string(),
            ))
            .expect("json");
        assert_eq!(reply["bilingual"], true);
        let staged = std::fs::read(reply["path"].as_str().expect("path")).expect("bytes");
        let transcript = store.get_transcript(id).expect("row");
        let expected =
            suramak_core::export::to_srt_bilingual(&transcript, &records).expect("stacked render");
        assert_eq!(staged, expected.as_bytes());
        assert!(String::from_utf8_lossy(&staged).contains("baris pertama"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cancelling_the_picker_returns_to_the_sheet_and_drops_the_stage() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        let (_, id) = fixture(&dir);
        let host = CompactHost::new(&dir);
        let _ = host.dispatch(
            &json!({ "op": "open_transcript", "transcript_id": id.to_string() }).to_string(),
        );
        let reply: Value = serde_json::from_str(
            &host.dispatch(&json!({ "op": "export", "format": "txt" }).to_string()),
        )
        .expect("json");
        let path = reply["path"].as_str().expect("path").to_string();
        let cancelled: Value = serde_json::from_str(
            &host.dispatch(&json!({ "op": "export_cancel", "job": reply["job"] }).to_string()),
        )
        .expect("json");
        assert!(cancelled["export"].is_null());
        assert_eq!(cancelled["sheet"], "export");
        assert!(!std::path::Path::new(&path).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_reports_the_reason_and_releases_the_stage() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        let (_, id) = fixture(&dir);
        let host = CompactHost::new(&dir);
        let _ = host.dispatch(
            &json!({ "op": "open_transcript", "transcript_id": id.to_string() }).to_string(),
        );
        let reply: Value = serde_json::from_str(
            &host.dispatch(&json!({ "op": "export", "format": "vtt" }).to_string()),
        )
        .expect("json");
        let path = reply["path"].as_str().expect("path").to_string();
        let failed: Value = serde_json::from_str(
            &host.dispatch(
                &json!({
                    "op": "export_failed",
                    "job": reply["job"],
                    "error": "the destination has 3 bytes; the export is 120",
                })
                .to_string(),
            ),
        )
        .expect("json");
        assert_eq!(failed["export"]["phase"], "failed");
        assert_eq!(
            failed["status"]["text"],
            "the destination has 3 bytes; the export is 120"
        );
        assert!(
            !std::path::Path::new(&path).exists(),
            "the stage is released"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_device_probe_builds_the_m13d_model_table() {
        let dir =
            std::env::temp_dir().join(format!("suramak-compact-host-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("models")).expect("models");
        std::fs::write(
            dir.join("models").join("ggml-base.bin"),
            b"not a model, just a filename",
        )
        .expect("model file");
        let host = CompactHost::new(&dir);
        let reply: Value = serde_json::from_str(
            &host.dispatch(
                &json!({
                    "op": "device",
                    "total_ram_bytes": 6_000_000_000_u64,
                    "available_ram_bytes": 2_000_000_000_u64,
                    "low_memory": false,
                    "arm64": true,
                })
                .to_string(),
            ),
        )
        .expect("json");
        let models = reply["models"].as_array().expect("models");
        let base = models
            .iter()
            .find(|row| row["id"] == "base")
            .expect("base row");
        assert_eq!(base["installed"], true);
        assert_eq!(base["recommended"], true);
        let q8 = models
            .iter()
            .find(|row| row["id"] == "turbo-q8-0")
            .expect("q8 row");
        assert_eq!(q8["offered"], false);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn back_before_the_host_exists_is_a_leave() {
        // Every other test in this binary drives a LOCAL CompactHost, so the
        // process-wide host is unset here and this exercises the None branch.
        // Back on the splash must leave: swallowing the gesture trapped the
        // smoke's Activity (alive 20 s after BACK, no finish-path exit).
        let reply: Value = serde_json::from_str(&dispatch(r#"{"op":"back"}"#)).expect("json");
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["finish"], true);
        // Every other op still fails closed without a host.
        let err: Value = serde_json::from_str(&dispatch(r#"{"op":"library"}"#)).expect("json");
        assert_eq!(err["ok"], false);
    }
}
