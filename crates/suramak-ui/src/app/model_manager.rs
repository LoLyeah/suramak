//! Model-manager logic — **egui-free** (unit-tested in this file): the manifest
//! table's install/encoder states (M1.5 helpers), delete plans honouring the
//! shared-encoder rule, the set-default write, and download progress math.
//!
//! The view (`app::models`) only renders [`ModelManager`] and requests effects;
//! `crate::runtime` executes them (download thread, file deletes, settings save).

use std::path::{Path, PathBuf};

use suramak_core::models::{self, ModelInfo};
use suramak_core::settings::Settings;

/// The message `suramak-core` returns for a cancelled transfer
/// ([`SuramakError::Cancelled`]'s display). Matched exactly so a cancel is
/// reported as a cancel, never as a download failure.
pub const CANCELLED: &str = "cancelled";

/// Install state of one manifest model's decoder file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallState {
    /// Neither the model file nor a `.part` resume file is present.
    Missing,
    /// A partial download (`.part`) is present.
    Partial,
    /// The model file is present.
    Installed,
}

impl InstallState {
    pub fn label(self) -> &'static str {
        match self {
            InstallState::Missing => "missing",
            InstallState::Partial => "partial",
            InstallState::Installed => "installed",
        }
    }
}

/// State of a model's official Core ML encoder package (M1.5 helpers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderState {
    /// Upstream ships no encoder for this model.
    NotAvailable,
    /// No encoder directory and no downloaded `.zip`.
    Missing,
    /// A `.zip` (or an incomplete directory) is present.
    Partial,
    /// The extracted package has every required file.
    Installed,
}

impl EncoderState {
    pub fn label(self) -> &'static str {
        match self {
            EncoderState::NotAvailable => "n/a",
            EncoderState::Missing => "missing",
            EncoderState::Partial => "partial",
            EncoderState::Installed => "installed",
        }
    }
}

/// One table row of the model manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelRow {
    pub id: &'static str,
    pub hf_file: &'static str,
    pub size_bytes: u64,
    pub role: &'static str,
    pub install: InstallState,
    pub encoder: EncoderState,
    /// `settings.model_id` currently points at this model.
    pub is_default: bool,
}

/// Background download state (progress comes from `download_with_progress`).
///
/// M14e: a download may be a **set** of files (the Parakeet model is four
/// pinned artifacts), so a running download also reports which file it is on
/// and how many there are — the Settings install card says "file 2 of 4".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DownloadState {
    #[default]
    Idle,
    Running {
        id: String,
        bytes_done: u64,
        bytes_total: u64,
        /// Files finished so far in a multi-file set (0 for a single file).
        files_done: u32,
        /// Files in the set (1 for a single-file model).
        files_total: u32,
        /// The file currently transferring (its manifest name), when known.
        file: Option<String>,
        /// The Cancel control was pressed: the stream stops at its next chunk.
        cancelling: bool,
    },
    Done {
        id: String,
    },
    Failed {
        id: String,
        message: String,
    },
    /// The user cancelled: the partial bytes stay on disk and resume next time.
    Cancelled {
        id: String,
    },
}

impl DownloadState {
    /// 0–100 while running; `None` otherwise.
    pub fn percent(&self) -> Option<u8> {
        match self {
            DownloadState::Running {
                bytes_done,
                bytes_total,
                ..
            } => {
                if *bytes_total == 0 {
                    Some(0)
                } else {
                    Some((bytes_done.min(bytes_total) * 100 / bytes_total) as u8)
                }
            }
            _ => None,
        }
    }

    /// The model id currently downloading, if any.
    pub fn active_id(&self) -> Option<&str> {
        match self {
            DownloadState::Running { id, .. } => Some(id),
            _ => None,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self, DownloadState::Running { .. })
    }

    /// "file 2 of 4 · encoder.int8.onnx" for a multi-file set, `None` for a
    /// single-file model (whose row already names itself).
    pub fn file_line(&self) -> Option<String> {
        match self {
            DownloadState::Running {
                files_total,
                files_done,
                file,
                ..
            } if *files_total > 1 || file.is_some() => Some(match file {
                Some(file) => format!(
                    "file {} of {} · {file}",
                    (files_done + 1).min(*files_total),
                    files_total
                ),
                None => format!("file {} of {}", files_done.min(files_total), files_total),
            }),
            _ => None,
        }
    }

    /// The Cancel control is in flight (the label says `cancelling…`).
    pub fn is_cancelling(&self) -> bool {
        matches!(
            self,
            DownloadState::Running {
                cancelling: true,
                ..
            }
        )
    }
}

/// Everything the model-manager view renders.
#[derive(Debug, Clone, Default)]
pub struct ModelManager {
    pub rows: Vec<ModelRow>,
    pub download: DownloadState,
    /// Last action result (Ok note / Err message), shown under the table.
    pub status: Option<Result<String, String>>,
    /// M9c motion: the smoothed download-bar position (chunked progress
    /// reports ease into one continuous move — no jitter).
    pub progress_ease: ProgressEase,
    /// M9c motion: bumped on every table load; rows play their base-token
    /// arrival once per epoch (the re-check / delete / download-done
    /// transition is seen, not teleported).
    pub refresh_epoch: u64,
}

/// The download bar's smoothing (M9c): each frame the shown value approaches
/// the reported fraction by a bounded step; it converges exactly, so the
/// animation ends (and never jitters between two chunk reports).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProgressEase {
    shown: f32,
}

impl ProgressEase {
    /// The smoothing's time constant: the bar covers ~63 % of a jump in this
    /// time. Sized above the download poll cadence (150 ms) so each chunk
    /// report moves the bar by a visible fraction instead of snapping.
    pub const TAU: f32 = 0.30;
    /// Settle threshold: within this the value snaps to the target (the
    /// animation ENDS — no perpetual sub-pixel motion).
    pub const EPSILON: f32 = 0.002;

    /// The current displayed fraction (0..=1).
    pub fn shown(&self) -> f32 {
        self.shown
    }

    /// Approaches `target` over `dt` seconds; `reduce` (Reduce Motion) snaps.
    /// Monotonic per target; the value lands exactly on the target.
    pub fn approach(&mut self, target: f32, dt: f32, reduce: bool) -> f32 {
        let target = target.clamp(0.0, 1.0);
        if reduce || dt <= 0.0 {
            self.shown = target;
            return self.shown;
        }
        let delta = target - self.shown;
        if delta.abs() <= Self::EPSILON {
            self.shown = target;
            return self.shown;
        }
        // Linear-in-time approach: never overshoots, never oscillates.
        let step = delta * (dt / Self::TAU).min(1.0);
        self.shown += step;
        if (target - self.shown).abs() <= Self::EPSILON {
            self.shown = target;
        }
        self.shown
    }
}

/// The human display name for a model id (M9c copy sweep — no quant-suffix
/// strings on screen). Unknown ids (a custom path, `auto`) pass through
/// unchanged; the raw id stays visible as the model manager's detail line.
pub fn display_name(id: &str) -> String {
    match id.trim() {
        "tiny" => "Tiny (fastest)".to_string(),
        "base" => "Base (legacy)".to_string(),
        "small" => "Small (fast)".to_string(),
        "turbo-q5-0" => "Large v3 Turbo (balanced)".to_string(),
        "turbo-q8-0" => "Large v3 Turbo (high quality)".to_string(),
        "turbo" => "Large v3 Turbo (full precision)".to_string(),
        // M14e: the fast engine's model in the same copy system (the Settings
        // readiness row and the install card both use it).
        "parakeet" => "Parakeet (fast engine)".to_string(),
        other => other.to_string(),
    }
}

impl ModelManager {
    /// Marks that a download of `id` is starting (the runtime does the work).
    ///
    /// `id` is a whisper manifest id or `parakeet` (the fast engine's model
    /// directory); the totals come from the pins either way.
    pub fn begin_download(&mut self, id: &str) -> bool {
        if self.download.is_running() {
            return false;
        }
        let (total, files) = match models::find_parakeet(id) {
            Some(info) => (models::parakeet_total_bytes(), info.files.len() as u32),
            None => (
                models::find_model(id)
                    .map(|info| info.size_bytes)
                    .unwrap_or(0),
                1,
            ),
        };
        self.download = DownloadState::Running {
            id: id.to_string(),
            bytes_done: 0,
            bytes_total: total,
            files_done: 0,
            files_total: files,
            file: None,
            cancelling: false,
        };
        // A fresh download restarts the smoothed bar at the bottom.
        self.progress_ease = ProgressEase::default();
        self.status = None;
        true
    }

    /// M9c: a fresh table arrived — rows play their arrival once (the epoch
    /// makes each refresh a deliberate transition).
    pub fn on_rows_loaded(&mut self, rows: Vec<ModelRow>) {
        self.rows = rows;
        self.refresh_epoch = self.refresh_epoch.wrapping_add(1);
    }

    /// Progress report from the runtime.
    pub fn on_progress(&mut self, bytes_done: u64, bytes_total: u64) {
        if let DownloadState::Running {
            bytes_done: done,
            bytes_total: total,
            ..
        } = &mut self.download
        {
            *done = bytes_done;
            *total = bytes_total.max(*total);
        }
    }

    /// M14e: per-file progress (the Parakeet install is four files). The
    /// totals keep their maximum so a resumed file reporting a smaller offset
    /// can never make the bar jump backwards.
    pub fn on_file_progress(
        &mut self,
        bytes_done: u64,
        bytes_total: u64,
        files_done: u32,
        files_total: u32,
        file: Option<&str>,
    ) {
        if let DownloadState::Running {
            bytes_done: done,
            bytes_total: total,
            files_done: done_files,
            files_total: total_files,
            file: current,
            ..
        } = &mut self.download
        {
            *done = bytes_done;
            *total = bytes_total.max(*total);
            *done_files = files_done;
            *total_files = files_total.max(*total_files);
            *current = file.map(str::to_string);
        }
    }

    /// The Cancel control: the transfer stops at its next chunk.
    pub fn begin_cancel(&mut self) -> bool {
        if let DownloadState::Running { cancelling, .. } = &mut self.download {
            *cancelling = true;
            return true;
        }
        false
    }

    /// The runtime confirmed the cancel (the partial bytes stay on disk).
    pub fn on_download_cancelled(&mut self, id: &str) {
        self.download = DownloadState::Cancelled { id: id.to_string() };
        self.status = Some(Ok(format!(
            "{} download cancelled — the partial files stay, so the next try resumes",
            display_name(id)
        )));
    }

    /// Terminal result of the download.
    pub fn on_download_done(&mut self, id: &str, result: Result<(), String>) {
        match &result {
            Ok(()) => {
                self.download = DownloadState::Done { id: id.to_string() };
                self.status = Some(Ok(format!(
                    "{} downloaded and sha256-verified",
                    display_name(id)
                )));
            }
            // A cancel is not a failure: core reports it with the exact
            // `cancelled` message, and the copy says what survived.
            Err(err) if err == CANCELLED => self.on_download_cancelled(id),
            Err(err) => {
                self.download = DownloadState::Failed {
                    id: id.to_string(),
                    message: err.clone(),
                };
                self.status = Some(Err(format!(
                    "download of {} failed: {err}",
                    display_name(id)
                )));
            }
        }
    }
}

/// `<dest>.part` — the temp file the core downloader resumes from.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut raw = dest.as_os_str().to_owned();
    raw.push(".part");
    PathBuf::from(raw)
}

/// Install state of `info`'s decoder file in `models_dir`.
pub fn install_state(models_dir: &Path, info: &ModelInfo) -> InstallState {
    let dest = models_dir.join(info.hf_file);
    if dest.is_file() {
        InstallState::Installed
    } else if part_path(&dest).is_file() {
        InstallState::Partial
    } else {
        InstallState::Missing
    }
}

/// Encoder-package state of `info` in `models_dir` (M1.5 helpers).
pub fn encoder_state(models_dir: &Path, info: &ModelInfo) -> EncoderState {
    let Some(package) = info.encoder else {
        return EncoderState::NotAvailable;
    };
    let dir = models_dir.join(models::encoder_dir_name(info.hf_file));
    if models::encoder_dir_ready(&dir) {
        EncoderState::Installed
    } else if dir.exists() || models_dir.join(package.zip_file).is_file() {
        EncoderState::Partial
    } else {
        EncoderState::Missing
    }
}

/// The full table for the manifest (`default_id` marks `settings.model_id`).
pub fn rows(models_dir: &Path, default_id: &str) -> Vec<ModelRow> {
    models::MODELS
        .iter()
        .map(|info| ModelRow {
            id: info.id,
            hf_file: info.hf_file,
            size_bytes: info.size_bytes,
            role: info.role,
            install: install_state(models_dir, info),
            encoder: encoder_state(models_dir, info),
            is_default: info.id == default_id.trim(),
        })
        .collect()
}

/// What deleting `id` would remove (a pure plan; [`apply_delete`] executes it).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletePlan {
    /// Decoder file and/or `.part` resume file that exist.
    pub decoder: Vec<PathBuf>,
    /// The encoder directory to remove, when no installed decoder still needs it.
    pub encoder_dir: Option<PathBuf>,
    /// Installed models that keep the shared encoder alive (M1.5 rule).
    pub encoder_kept_for: Vec<String>,
    /// M14e: the Parakeet model directory (a set of pinned files, not a single
    /// decoder) — removed whole, with any `.part` files inside it.
    pub parakeet_dir: Option<PathBuf>,
}

/// Builds the delete plan for `id`, honouring the shared-encoder rule: all three
/// turbo decoders pair with one encoder package, so it only goes when the last
/// decoder that needs it goes.
///
/// `parakeet` (M14e) is the fast engine's model directory: the plan removes the
/// whole directory, partial files included — the same thing the CLI's
/// `models remove parakeet` does.
pub fn delete_plan(models_dir: &Path, id: &str) -> Result<DeletePlan, String> {
    if models::find_parakeet(id).is_some() {
        let dir = models::parakeet_dir(models_dir);
        return Ok(DeletePlan {
            parakeet_dir: dir.exists().then_some(dir),
            ..DeletePlan::default()
        });
    }
    let info = models::find_model(id).ok_or_else(|| format!("unknown model '{id}'"))?;
    let dest = models_dir.join(info.hf_file);
    let decoder = [dest.clone(), part_path(&dest)]
        .into_iter()
        .filter(|path| path.exists())
        .collect();

    let mut plan = DeletePlan {
        decoder,
        encoder_dir: None,
        encoder_kept_for: Vec::new(),
        parakeet_dir: None,
    };
    let dir_name = models::encoder_dir_name(info.hf_file);
    let dir = models_dir.join(&dir_name);
    if dir.exists() {
        plan.encoder_kept_for = models::MODELS
            .iter()
            .filter(|other| other.id != info.id)
            .filter(|other| models::encoder_dir_name(other.hf_file) == dir_name)
            .filter(|other| models_dir.join(other.hf_file).exists())
            .map(|other| other.id.to_string())
            .collect();
        if plan.encoder_kept_for.is_empty() {
            plan.encoder_dir = Some(dir);
        }
    }
    Ok(plan)
}

/// Executes a [`DeletePlan`]; returns the removed paths.
///
/// A Parakeet directory reports the **files** it contained (so the status line
/// reads "removed 4 files" rather than "removed 1 file" for a directory).
pub fn apply_delete(plan: &DeletePlan) -> Result<Vec<PathBuf>, String> {
    let mut removed = Vec::new();
    for path in &plan.decoder {
        std::fs::remove_file(path)
            .map_err(|err| format!("cannot remove {}: {err}", path.display()))?;
        removed.push(path.clone());
    }
    if let Some(dir) = &plan.encoder_dir {
        std::fs::remove_dir_all(dir)
            .map_err(|err| format!("cannot remove {}: {err}", dir.display()))?;
        removed.push(dir.clone());
    }
    if let Some(dir) = &plan.parakeet_dir {
        // Collect what is actually there before removing the directory (the
        // report counts real files, never the manifest's four).
        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|err| format!("cannot read {}: {err}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        files.sort();
        std::fs::remove_dir_all(dir)
            .map_err(|err| format!("cannot remove {}: {err}", dir.display()))?;
        removed.extend(files);
    }
    Ok(removed)
}

/// Applies the "set default" action: `settings.model_id = id` (rejects unknown ids).
pub fn apply_default(settings: &mut Settings, id: &str) -> Result<(), String> {
    if models::find_model(id).is_none() {
        return Err(format!("unknown model '{id}'"));
    }
    settings.asr.model_id = id.to_string();
    Ok(())
}

/// Human-readable decimal size, e.g. `77.7 MB` (same shape as the CLI's).
pub fn human_size(bytes: u64) -> String {
    const KB: f64 = 1_000.0;
    const MB: f64 = 1_000_000.0;
    const GB: f64 = 1_000_000_000.0;
    let b = bytes as f64;
    if b >= GB {
        format!("{:.1} GB", b / GB)
    } else if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_models_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-models-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp models dir");
        dir
    }

    /// Materializes a complete encoder directory for `info`.
    fn install_encoder_dir(models_dir: &Path, info: &ModelInfo) {
        let dir = models_dir.join(models::encoder_dir_name(info.hf_file));
        for file in models::ENCODER_REQUIRED_FILES {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
    }

    #[test]
    fn rows_report_install_and_encoder_states() {
        let dir = temp_models_dir("rows");
        // tiny: installed + full encoder.
        std::fs::write(dir.join("ggml-tiny.bin"), b"x").unwrap();
        install_encoder_dir(&dir, models::find_model("tiny").unwrap());
        // small: partial download + downloaded-but-unpacked encoder zip.
        std::fs::write(dir.join("ggml-small.bin.part"), b"x").unwrap();
        std::fs::write(
            dir.join(
                models::find_model("small")
                    .unwrap()
                    .encoder
                    .unwrap()
                    .zip_file,
            ),
            b"x",
        )
        .unwrap();

        let table = rows(&dir, "tiny");
        let by_id = |id: &str| table.iter().find(|r| r.id == id).unwrap();
        assert_eq!(by_id("tiny").install, InstallState::Installed);
        assert_eq!(by_id("tiny").encoder, EncoderState::Installed);
        assert_eq!(by_id("small").install, InstallState::Partial);
        assert_eq!(by_id("small").encoder, EncoderState::Partial);
        assert_eq!(by_id("base").install, InstallState::Missing);
        assert_eq!(by_id("base").encoder, EncoderState::Missing);
        assert_eq!(by_id("turbo").install, InstallState::Missing);
        // A model whose "file" is a directory is not installed.
        std::fs::create_dir_all(dir.join("ggml-base.bin")).unwrap();
        let table = rows(&dir, "tiny");
        assert_eq!(
            table.iter().find(|r| r.id == "base").unwrap().install,
            InstallState::Missing
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rows_mark_the_default_model() {
        let dir = temp_models_dir("default");
        let table = rows(&dir, "small");
        assert_eq!(table.iter().filter(|r| r.is_default).count(), 1);
        assert_eq!(table.iter().find(|r| r.is_default).unwrap().id, "small");
        // "auto" (or a path) marks nothing.
        assert!(rows(&dir, "auto").iter().all(|r| !r.is_default));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_plan_keeps_an_encoder_shared_by_installed_turbo_models() {
        let dir = temp_models_dir("shared");
        std::fs::write(dir.join("ggml-large-v3-turbo-q5_0.bin"), b"x").unwrap();
        std::fs::write(dir.join("ggml-large-v3-turbo-q8_0.bin"), b"x").unwrap();
        let turbo = models::find_model("turbo-q5-0").unwrap();
        install_encoder_dir(&dir, turbo);

        let plan = delete_plan(&dir, "turbo-q5-0").expect("plan");
        assert_eq!(plan.decoder.len(), 1);
        assert_eq!(
            plan.encoder_dir, None,
            "the encoder must survive for the remaining turbo decoder"
        );
        assert_eq!(plan.encoder_kept_for, vec!["turbo-q8-0".to_string()]);

        // Executing the plan leaves the shared encoder + q8 decoder in place.
        let removed = apply_delete(&plan).expect("apply");
        assert_eq!(removed.len(), 1);
        assert!(!dir.join("ggml-large-v3-turbo-q5_0.bin").exists());
        assert!(dir.join("ggml-large-v3-turbo-q8_0.bin").exists());
        assert!(models::encoder_dir_ready(
            &dir.join(models::encoder_dir_name(turbo.hf_file))
        ));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_plan_removes_an_unshared_encoder_and_partial_files() {
        let dir = temp_models_dir("unshared");
        let info = models::find_model("turbo").unwrap();
        // A never-finished download: only the .part exists, no model file.
        std::fs::write(dir.join(format!("{}.part", info.hf_file)), b"x").unwrap();
        install_encoder_dir(&dir, info);

        let plan = delete_plan(&dir, "turbo").expect("plan");
        assert_eq!(plan.decoder.len(), 1, "the .part is collected");
        assert_eq!(
            plan.encoder_dir,
            Some(dir.join(models::encoder_dir_name(info.hf_file)))
        );
        assert!(plan.encoder_kept_for.is_empty());

        let removed = apply_delete(&plan).expect("apply");
        assert_eq!(removed.len(), 2);
        assert!(
            !dir.join(models::encoder_dir_name(info.hf_file)).exists(),
            "the unshared encoder directory is gone"
        );

        // An unknown id is an error, not a silent no-op.
        assert!(delete_plan(&dir, "nope").is_err());
        assert!(delete_plan(&dir, "tiny").expect("plan").decoder.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn set_default_writes_model_id_and_persists() {
        let mut settings = Settings::default();
        apply_default(&mut settings, "small").expect("set default");
        assert_eq!(settings.asr.model_id, "small");

        // The write is a real toml round-trip (PLAN's observable-settings rule).
        let dir = temp_models_dir("set-default");
        let path = dir.join("settings.toml");
        settings.save(&path).expect("save settings");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("model_id = \"small\""), "unexpected: {text}");
        let back = Settings::load(&path).expect("reload");
        assert_eq!(back.asr.model_id, "small");

        assert!(apply_default(&mut settings, "not-a-model").is_err());
        assert_eq!(
            settings.asr.model_id, "small",
            "a failed set changes nothing"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn download_progress_reports_percent_and_terminal_states() {
        let mut manager = ModelManager::default();
        assert_eq!(manager.download.percent(), None);

        // idle → a begin starts, pulling the total from the manifest.
        assert!(manager.begin_download("tiny"));
        match &manager.download {
            DownloadState::Running {
                id, bytes_total, ..
            } => {
                assert_eq!(id, "tiny");
                assert_eq!(*bytes_total, 77_691_713);
            }
            other => panic!("expected a running download, got {other:?}"),
        }
        // A running download rejects another begin.
        assert!(!manager.begin_download("base"));
        assert_eq!(manager.download.active_id(), Some("tiny"));

        manager.on_progress(38_845_856, 77_691_713);
        assert_eq!(manager.download.percent(), Some(49));

        manager.on_download_done("tiny", Err("checksum mismatch".to_string()));
        assert!(matches!(manager.download, DownloadState::Failed { .. }));
        assert!(manager.status.as_ref().unwrap().is_err());

        manager.begin_download("tiny");
        assert!(manager.download.is_running());
        manager.on_download_done("tiny", Ok(()));
        assert!(matches!(manager.download, DownloadState::Done { .. }));
        let status = manager.status.as_ref().unwrap().as_ref().unwrap();
        assert!(status.contains("sha256-verified"), "unexpected: {status}");

        // Zero-total (unknown size) never divides by zero; done → None.
        manager.begin_download("base");
        manager.on_progress(5, 0);
        assert_eq!(manager.download.percent(), Some(0));
    }

    // ---- M14e: the Parakeet install through the same manager -----------------

    #[test]
    fn a_parakeet_download_reports_the_manifest_total_and_its_file_set() {
        let mut manager = ModelManager::default();
        assert!(manager.begin_download("parakeet"));
        match &manager.download {
            DownloadState::Running {
                id,
                bytes_total,
                files_total,
                files_done,
                file,
                cancelling,
                ..
            } => {
                assert_eq!(id, "parakeet");
                assert_eq!(
                    *bytes_total,
                    models::parakeet_total_bytes(),
                    "the total is the sum of the pins (~670 MB)"
                );
                assert_eq!(*files_total, 4);
                assert_eq!(*files_done, 0);
                assert_eq!(*file, None);
                assert!(!cancelling);
            }
            other => panic!("expected a running download, got {other:?}"),
        }
        // The single-file models keep files_total = 1 and no file line.
        let mut whisper = ModelManager::default();
        assert!(whisper.begin_download("tiny"));
        assert_eq!(whisper.download.file_line(), None);

        // Per-file progress: the card names the file and counts the set.
        manager.on_file_progress(
            100_000,
            models::parakeet_total_bytes(),
            1,
            4,
            Some("encoder.int8.onnx"),
        );
        assert_eq!(
            manager.download.file_line().as_deref(),
            Some("file 2 of 4 · encoder.int8.onnx")
        );
        // The byte total never shrinks when a resumed file reports an offset.
        manager.on_file_progress(50_000, 10, 1, 4, Some("encoder.int8.onnx"));
        match &manager.download {
            DownloadState::Running { bytes_total, .. } => {
                assert_eq!(*bytes_total, models::parakeet_total_bytes())
            }
            other => panic!("expected a running download, got {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_download_is_not_a_failure() {
        let mut manager = ModelManager::default();
        assert!(manager.begin_download("parakeet"));
        assert!(manager.begin_cancel(), "the Cancel control arms the stop");
        assert!(manager.download.is_cancelling());

        manager.on_download_done("parakeet", Err(CANCELLED.to_string()));
        assert!(matches!(manager.download, DownloadState::Cancelled { .. }));
        let status = manager.status.as_ref().expect("a status line");
        let note = status.as_ref().expect("a cancel is not an error");
        assert!(note.contains("cancelled"), "unexpected: {note}");
        assert!(
            note.contains("resumes"),
            "the copy says the partial files survive: {note}"
        );

        // A real failure still reads as a failure.
        let mut failed = ModelManager::default();
        assert!(failed.begin_download("parakeet"));
        failed.on_download_done("parakeet", Err("checksum mismatch".to_string()));
        assert!(matches!(failed.download, DownloadState::Failed { .. }));
        assert!(failed.status.as_ref().unwrap().is_err());
        // …and an idle manager cannot arm a cancel.
        let mut idle = ModelManager::default();
        assert!(!idle.begin_cancel());
    }

    #[test]
    fn deleting_parakeet_removes_its_directory_and_counts_its_files() {
        let dir = temp_models_dir("parakeet-delete");
        std::fs::create_dir_all(models::parakeet_dir(&dir)).unwrap();
        for file in models::PARAKEET.files {
            std::fs::write(models::parakeet_dir(&dir).join(file.name), b"x").unwrap();
        }
        // A partial file from an interrupted attempt goes with the directory.
        std::fs::write(
            models::parakeet_dir(&dir).join("decoder.int8.onnx.part"),
            b"x",
        )
        .unwrap();

        let plan = delete_plan(&dir, "parakeet").expect("a parakeet plan");
        assert_eq!(plan.decoder.len(), 0, "no whisper decoder is involved");
        assert_eq!(plan.encoder_dir, None);
        assert_eq!(plan.parakeet_dir, Some(models::parakeet_dir(&dir)));

        let removed = apply_delete(&plan).expect("apply");
        assert_eq!(removed.len(), 5, "the four pinned files + the .part");
        assert!(!models::parakeet_dir(&dir).exists());

        // Nothing installed → an empty plan, not an error.
        let again = delete_plan(&dir, "parakeet").expect("a plan");
        assert_eq!(again.parakeet_dir, None);
        assert!(apply_delete(&again).unwrap().is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn human_size_matches_the_cli_shape() {
        assert_eq!(human_size(500), "500 B");
        assert_eq!(human_size(77_691_713), "77.7 MB");
        assert_eq!(human_size(1_624_555_275), "1.6 GB");
    }

    // ---- M9c: display names + smoothed progress ------------------------------

    #[test]
    fn every_manifest_model_has_a_human_display_name() {
        for info in models::MODELS {
            let name = display_name(info.id);
            assert_ne!(name, info.id, "{} must have a human label", info.id);
            assert!(!name.is_empty());
            // No quant-suffix / raw-id strings on screen (brief principle 7).
            for leak in ["q5", "q8", "ggml", "_0", "-q"] {
                assert!(
                    !name.contains(leak),
                    "{name:?} leaks the raw quant suffix {leak:?}"
                );
            }
        }
        // The owner's example maps exactly.
        assert_eq!(display_name("turbo-q5-0"), "Large v3 Turbo (balanced)");
        assert_eq!(display_name("turbo-q8-0"), "Large v3 Turbo (high quality)");
        assert_eq!(display_name("small"), "Small (fast)");
        // Unknown ids (custom paths, `auto`) pass through untouched.
        assert_eq!(display_name("auto"), "auto");
        assert_eq!(display_name("/models/custom.bin"), "/models/custom.bin");
        assert_eq!(display_name(" turbo-q5-0 "), "Large v3 Turbo (balanced)");
    }

    #[test]
    fn progress_ease_is_monotonic_settles_and_snaps_under_reduce_motion() {
        let mut ease = ProgressEase::default();
        assert_eq!(ease.shown(), 0.0);

        // Approaching 0.5 in small steps: monotonic, bounded, never overshoots,
        // and it settles exactly (the ease ENDS).
        let mut last = 0.0;
        for _ in 0..600 {
            let value = ease.approach(0.5, 1.0 / 60.0, false);
            assert!(value >= last && value <= 0.5 + f32::EPSILON, "{value}");
            last = value;
        }
        assert_eq!(ease.shown(), 0.5, "the ease ends exactly on the target");

        // A chunked report mid-flight does not jitter: the value keeps moving
        // toward the new target without a jump backwards, and one download
        // cadence frame (150 ms) moves a visible fraction of the gap.
        let before = ease.shown();
        let after = ease.approach(0.7, 0.150, false);
        assert!(
            after > before + 0.05 && after <= 0.7,
            "a poll moves the bar visibly: {before} -> {after}"
        );

        // Reduce Motion: a snap.
        let mut reduced = ProgressEase::default();
        assert_eq!(reduced.approach(0.42, 1.0 / 60.0, true), 0.42);
        assert_eq!(reduced.approach(0.9, 1.0 / 60.0, true), 0.9);
        // Out-of-range targets clamp.
        assert_eq!(reduced.approach(2.0, 1.0 / 60.0, true), 1.0);
        assert_eq!(reduced.approach(-1.0, 1.0 / 60.0, true), 0.0);
    }

    #[test]
    fn a_refresh_bumps_the_epoch_so_rows_replay_their_arrival() {
        let mut manager = ModelManager::default();
        assert_eq!(manager.refresh_epoch, 0);
        manager.on_rows_loaded(Vec::new());
        manager.on_rows_loaded(Vec::new());
        assert_eq!(
            manager.refresh_epoch, 2,
            "every load is a new arrival epoch"
        );
        // A fresh download restarts the smoothed bar.
        let mut ease = ProgressEase::default();
        ease.approach(0.8, 1.0, true);
        assert_eq!(ease.shown(), 0.8);
        manager.progress_ease = ease;
        assert!(manager.begin_download("tiny"));
        assert_eq!(manager.progress_ease.shown(), 0.0);
    }
}
