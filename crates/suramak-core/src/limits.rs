//! M26A — limits that come from the device, not from a 3.7 GiB VPS.
//!
//! The M25B caps (45 min on Android, 60 min on desktop) were artifacts of the
//! box the measurements were taken on. This module is the single source of
//! truth that replaces them:
//!
//! * [`compute_device_limits`] — pure arithmetic over a device's memory report,
//!   the model's measured footprint and the free temp disk. It returns
//!   [`Limits`]: the duration cap, whether the model fits at all, the byte
//!   budget the cap came from, and which rule set the cap.
//! * [`MODEL_FOOTPRINTS`] — **one** table of model baselines and per-minute
//!   slopes, each row's comment naming the measurement it comes from.
//! * Platform probes with **no new crates**: `/proc/meminfo` on Linux (and
//!   Android, where the Java `DeviceMemory.Report` overrides it),
//!   `sysctlbyname("hw.memsize")` on macOS, and a raw
//!   `GlobalMemoryStatusEx` FFI on Windows.
//!
//! A probe that fails is never a silent guess: [`limits_for_model`] logs the
//! named failure and returns [`FALLBACK_MAX_MINUTES`] with
//! [`CapReason::Fallback`], the documented conservative default.
//!
//! The formulas are the design's (`docs/reports/M26-memory-design.md` §D):
//!
//! ```text
//! Android: low_ram || total <= 3 GiB -> reserve 1800 MB, frac 0.25
//!          total <= 4.5 GB           -> reserve 2100 MB, frac 0.30
//!          else                      -> reserve 2800 MB, frac 0.42
//!          B = min(total*frac, total-reserve); if available known, min(B, avail*0.65)
//! macOS    B = (total - 4096 MB) * 0.60
//! Linux    B = min(total*0.55, available*0.75)
//! Windows  B = min(total*0.55, avail_pagefile*0.65)
//! B_safe   = B * 0.85
//! max_minutes = clamp(floor((B_safe - model_base) / model_slope), 0, 480)
//! model_fits  = B_safe >= model_base + 15 * model_slope
//! ```
//!
//! All byte counts are converted to **decimal MB** (bytes / 1,000,000), the
//! unit the design's worked table and `/proc/meminfo` readings use.

use std::path::Path;

/// One MiB in bytes (the Android low-RAM boundary is defined in GiB).
const GIB: u64 = 1024 * 1024 * 1024;
/// The design's 4.5 GB Android tier boundary, in bytes (decimal GB).
const ANDROID_MID_TIER_BYTES: u64 = 4_500_000_000;
/// Safety factor applied to every platform budget (egui/wgpu/SQLite spikes).
const SAFETY: f64 = 0.85;
/// The practical UX ceiling: eight hours.
pub const MAX_MINUTES_CEILING: u32 = 480;
/// The conservative cap when no probe answers (`M26A` deliverable 2).
pub const FALLBACK_MAX_MINUTES: u32 = 60;
/// The design's disk headroom before the temp-file cap starts counting.
const DISK_HEADROOM_BYTES: u64 = 500 * 1_000_000;
/// A 16 kHz mono `f32` PCM minute, the design's disk-rate constant.
const DISK_MB_PER_MINUTE: f64 = 3.84;
/// The model-fit window: a model must fit for at least this many minutes.
const FIT_WINDOW_MINUTES: f64 = 15.0;
/// `1_039 MB` in the model-fit copy is a rounded measurement; keep the `~`.
const MODEL_BASE_UNIT: &str = "MB";

/// The platform whose budget formula applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    Android,
    MacOs,
    Linux,
    Windows,
}

impl Platform {
    /// The host this build is running on.
    pub const fn host() -> Self {
        if cfg!(target_os = "android") {
            Platform::Android
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// Stable lowercase name for logs and the refusal copy.
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Android => "android",
            Platform::MacOs => "macos",
            Platform::Linux => "linux",
            Platform::Windows => "windows",
        }
    }
}

/// One model's measured memory footprint.
///
/// `baseline_mb` is the peak with zero minutes of audio (weights + runtime),
/// `slope_mb_per_min` is the measured growth per audio minute. Both are
/// **decimal MB**, matching [`Platform`]'s byte conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelFootprint {
    pub id: &'static str,
    pub baseline_mb: u32,
    pub slope_mb_per_min: f32,
}

/// The **single** model table (M26 §D.1). Do not add a second one.
///
/// Provenance, per row:
/// * `tiny` — measured 308.4 MB intercept (M25A) and the measured
///   9.19 MB/min slope, rounded to the design's 310 / 9.2.
/// * `turbo-q5-0` — measured 1,038 MB intercept on the 574 MB `q5_0` file and
///   10.5 MB/min with `n_mel = 128` (M26 §D.1); rounded to 1,039 / 10.5.
/// * `base` and `small` — the design's §D.1 table values (380 / 700 MB on the
///   80-mel 9.2 MB/min slope). No direct intercept was measured for these two;
///   they come from the `max(310, model_file_mb * 1.81)` rule and the table.
/// * `turbo` — the full-precision `large-v3-turbo` pair (1,900 / 10.5) from the
///   same table (128-mel slope).
pub static MODEL_FOOTPRINTS: &[ModelFootprint] = &[
    ModelFootprint {
        id: "tiny",
        baseline_mb: 310,
        slope_mb_per_min: 9.2,
    },
    ModelFootprint {
        id: "base",
        baseline_mb: 380,
        slope_mb_per_min: 9.2,
    },
    ModelFootprint {
        id: "small",
        baseline_mb: 700,
        slope_mb_per_min: 9.2,
    },
    ModelFootprint {
        id: "turbo-q5-0",
        baseline_mb: 1_039,
        slope_mb_per_min: 10.5,
    },
    ModelFootprint {
        id: "turbo",
        baseline_mb: 1_900,
        slope_mb_per_min: 10.5,
    },
];

/// The footprint for a manifest id, if the table has one.
pub fn footprint_for(id: &str) -> Option<&'static ModelFootprint> {
    MODEL_FOOTPRINTS.iter().find(|model| model.id == id)
}

/// The footprint for a model *file path* (`…/ggml-tiny.bin` → `tiny`).
///
/// A path whose name is not in the table is not gated: the table only knows
/// the models this build ships, and refusing an unknown custom file by a
/// number we never measured would be a guess.
pub fn footprint_for_path(path: &Path) -> Option<&'static ModelFootprint> {
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let id = stem.strip_prefix("ggml-").unwrap_or(stem);
    footprint_for(id)
}

/// Which rule decided [`Limits::max_minutes`] — the refusal's own word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapReason {
    /// The RAM budget alone set the cap.
    Memory,
    /// Free temp disk was the tighter bound.
    Disk,
    /// The eight-hour UX ceiling clamped the cap.
    Ceiling,
    /// The model does not fit even 15 minutes in the budget.
    ModelTooLarge,
    /// No platform probe answered: the conservative 60-minute default.
    Fallback,
}

impl CapReason {
    /// The stable word the refusal and the job card carry.
    pub fn as_str(self) -> &'static str {
        match self {
            CapReason::Memory => "memory",
            CapReason::Disk => "disk",
            CapReason::Ceiling => "ceiling",
            CapReason::ModelTooLarge => "model too large",
            CapReason::Fallback => "fallback",
        }
    }
}

/// The computed limits for one (device, model) pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Limits {
    /// The duration cap in whole minutes, `0..=480`.
    pub max_minutes: u32,
    /// Whether the model fits at least [`FIT_WINDOW_MINUTES`] in `budget_mb`.
    pub model_fits: bool,
    /// `B_safe` — the byte budget the cap came from. `0` on the fallback path
    /// (no probe answered, so there is no measured budget to quote).
    pub budget_mb: u32,
    /// Which rule set [`Limits::max_minutes`].
    pub reason: CapReason,
}

impl Limits {
    /// The cap in milliseconds (the form the session/recorder code speaks).
    pub fn max_ms(self) -> i64 {
        i64::from(self.max_minutes) * 60_000
    }

    /// The sentence every refusal carries: the budget and the model's cost.
    ///
    /// ```text
    /// this device budgets 4,284 MB; tiny needs 310 MB + 9.2 MB per minute
    /// ```
    pub fn budget_line(self, model: &ModelFootprint) -> String {
        if self.reason == CapReason::Fallback {
            return format!(
                "the device memory probe did not answer, so this build uses its conservative \
                 {FALLBACK_MAX_MINUTES}-minute cap"
            );
        }
        format!(
            "this device budgets {} {unit}; {} needs {} {unit} + {} MB per minute",
            thousands(self.budget_mb),
            model.id,
            thousands(model.baseline_mb),
            slope_text(model.slope_mb_per_min),
            unit = MODEL_BASE_UNIT,
        )
    }
}

/// A device memory report. `available_bytes` means what the platform's formula
/// needs: `MemAvailable` on Linux/Android, `ullAvailPageFile` on Windows, and
/// `None` on macOS (its formula uses total memory only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryProbe {
    pub total_bytes: u64,
    pub available_bytes: Option<u64>,
    /// `ActivityManager.isLowRamDevice` (or `MemoryInfo.lowMemory`) on Android.
    pub low_ram: bool,
}

impl MemoryProbe {
    pub const fn new(total_bytes: u64, available_bytes: Option<u64>, low_ram: bool) -> Self {
        Self {
            total_bytes,
            available_bytes,
            low_ram,
        }
    }

    /// The probe from the Android host's `DeviceMemory.Report`.
    pub const fn from_android_report(
        total_bytes: u64,
        available_bytes: u64,
        low_ram: bool,
    ) -> Self {
        Self {
            total_bytes,
            available_bytes: if available_bytes == 0 {
                None
            } else {
                Some(available_bytes)
            },
            low_ram,
        }
    }
}

fn bytes_to_mb(bytes: u64) -> f64 {
    bytes as f64 / 1_000_000.0
}

#[allow(clippy::manual_is_multiple_of)]
fn thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn slope_text(slope: f32) -> String {
    format!("{slope:.1}")
}

/// The fallback limits: the conservative 60-minute cap, no measured budget.
pub fn fallback_limits() -> Limits {
    Limits {
        max_minutes: FALLBACK_MAX_MINUTES,
        model_fits: true,
        budget_mb: 0,
        reason: CapReason::Fallback,
    }
}

/// The platform's `B` (before the 0.85 safety factor), in MB.
fn platform_budget(
    platform: Platform,
    total_mb: f64,
    available_mb: Option<f64>,
    low_ram: bool,
    total_bytes: u64,
) -> f64 {
    match platform {
        Platform::Android => {
            let (reserve, fraction) = if low_ram || total_bytes <= 3 * GIB {
                (1_800.0, 0.25)
            } else if total_bytes <= ANDROID_MID_TIER_BYTES {
                (2_100.0, 0.30)
            } else {
                (2_800.0, 0.42)
            };
            let from_total = (total_mb * fraction).min((total_mb - reserve).max(0.0));
            match available_mb {
                Some(available) => from_total.min(available * 0.65),
                None => from_total,
            }
        }
        Platform::MacOs => (total_mb - 4_096.0).max(0.0) * 0.60,
        Platform::Linux => match available_mb {
            Some(available) => (total_mb * 0.55).min(available * 0.75),
            None => total_mb * 0.55,
        },
        Platform::Windows => match available_mb {
            Some(available) => (total_mb * 0.55).min(available * 0.65),
            None => total_mb * 0.55,
        },
    }
}

/// The disk cap: `floor((free - 500 MB) / 3.84 MB per minute)`, the design's
/// temp-file rule. `None` when free space is unknown (no disk bound).
fn disk_minutes(free_disk_bytes: Option<u64>) -> Option<u32> {
    let free = free_disk_bytes?;
    let usable_mb = bytes_to_mb(free.saturating_sub(DISK_HEADROOM_BYTES));
    Some((usable_mb / DISK_MB_PER_MINUTE).floor().max(0.0) as u32)
}

/// The pure formula. Every input is explicit, so the design's table is a unit
/// test on any host.
///
/// `available_bytes` is the platform's own "available" figure (see
/// [`MemoryProbe`]); `free_disk_bytes` bounds the temp PCM the design writes,
/// and `None` means "unknown" (no disk bound).
#[allow(clippy::too_many_arguments)]
pub fn compute_device_limits(
    platform: Platform,
    total_bytes: u64,
    available_bytes: Option<u64>,
    low_ram: bool,
    model_baseline_mb: u32,
    model_slope_mb_per_min: f32,
    free_disk_bytes: Option<u64>,
) -> Limits {
    if total_bytes == 0 {
        return fallback_limits();
    }
    let total_mb = bytes_to_mb(total_bytes);
    let available_mb = available_bytes.map(bytes_to_mb);
    let b = platform_budget(platform, total_mb, available_mb, low_ram, total_bytes);
    // A negative B (a tiny device under its OS reserve) is a zero budget, not
    // a negative one; the model-fit gate then refuses the model by name.
    let b_safe = (b * SAFETY).max(0.0);
    let budget_mb = (b_safe + 1e-9).floor() as u32;

    let slope = f64::from(model_slope_mb_per_min.max(0.1));
    let baseline = f64::from(model_baseline_mb);
    let model_fits = b_safe >= baseline + FIT_WINDOW_MINUTES * slope;

    let mut reason = if model_fits {
        CapReason::Memory
    } else {
        CapReason::ModelTooLarge
    };
    let mut minutes = (((b_safe - baseline) / slope).floor().max(0.0)) as u32;

    if let Some(disk) = disk_minutes(free_disk_bytes) {
        if disk < minutes {
            minutes = disk;
            reason = if model_fits {
                CapReason::Disk
            } else {
                CapReason::ModelTooLarge
            };
        }
    }
    if minutes > MAX_MINUTES_CEILING {
        minutes = MAX_MINUTES_CEILING;
        reason = CapReason::Ceiling;
    }

    Limits {
        max_minutes: minutes,
        model_fits,
        budget_mb,
        reason,
    }
}

/// [`compute_device_limits`] from a probe.
pub fn compute_for(
    platform: Platform,
    probe: MemoryProbe,
    model: &ModelFootprint,
    free_disk_bytes: Option<u64>,
) -> Limits {
    compute_device_limits(
        platform,
        probe.total_bytes,
        probe.available_bytes,
        probe.low_ram,
        model.baseline_mb,
        model.slope_mb_per_min,
        free_disk_bytes,
    )
}

/// Computes the limits from a probe **result**. A failed probe is the
/// documented fallback, logged by name — never a silent guess.
pub fn limits_from_probe(
    probe: Result<MemoryProbe, String>,
    platform: Platform,
    model: &ModelFootprint,
    free_disk_bytes: Option<u64>,
) -> Limits {
    match probe {
        Ok(probe) => compute_for(platform, probe, model, free_disk_bytes),
        Err(cause) => {
            tracing::warn!(
                platform = platform.as_str(),
                model = model.id,
                cause = %cause,
                "device memory probe failed; using the conservative {FALLBACK_MAX_MINUTES}-minute cap"
            );
            fallback_limits()
        }
    }
}

/// The limits for one model on this host, probing the platform.
pub fn limits_for_model(model: &ModelFootprint, free_disk_bytes: Option<u64>) -> Limits {
    limits_from_probe(probe_host(), Platform::host(), model, free_disk_bytes)
}

/// The limits for a model file path; an unknown path is budgeted with `tiny`
/// (the audio slope, not a model-size claim — see [`footprint_for_path`]).
pub fn limits_for_model_path(path: &Path, free_disk_bytes: Option<u64>) -> Limits {
    let model = footprint_for_path(path)
        .unwrap_or_else(|| footprint_for("tiny").expect("the model table always carries tiny"));
    limits_for_model(model, free_disk_bytes)
}

/// Free bytes on the volume the job's temp PCM lands on.
pub fn host_free_disk_bytes() -> Option<u64> {
    crate::download::available_space_bytes(&std::env::temp_dir())
}

/// The largest table model that fits `budget_mb` (the conservative choice for
/// a recording cap when the configured model itself does not fit).
pub fn largest_fitting(budget_mb: u32) -> Option<&'static ModelFootprint> {
    let fits = |model: &&ModelFootprint| {
        f64::from(budget_mb)
            >= f64::from(model.baseline_mb) + FIT_WINDOW_MINUTES * f64::from(model.slope_mb_per_min)
    };
    MODEL_FOOTPRINTS
        .iter()
        .filter(fits)
        .max_by_key(|model| model.baseline_mb)
}

/// The model-fit refusal, or `None` when the model fits.
///
/// ```text
/// model_too_large: turbo-q5-0 needs ~1,039 MB; this device budgets 1,020 MB — choose small or base
/// ```
///
/// The fix named is the **largest models that do fit**, never a silent swap:
/// the caller refuses and the user chooses.
pub fn model_fit_refusal(model: &ModelFootprint, limits: &Limits) -> Option<String> {
    if limits.model_fits {
        return None;
    }
    let mut fitting: Vec<&'static ModelFootprint> = MODEL_FOOTPRINTS
        .iter()
        .filter(|candidate| {
            candidate.id != model.id
                && f64::from(limits.budget_mb)
                    >= f64::from(candidate.baseline_mb)
                        + FIT_WINDOW_MINUTES * f64::from(candidate.slope_mb_per_min)
        })
        .collect();
    fitting.sort_by_key(|candidate| std::cmp::Reverse(candidate.baseline_mb));
    let choices = match fitting.as_slice() {
        [] => "a smaller model".to_string(),
        [only] => only.id.to_string(),
        [first, second, ..] => format!("{} or {}", first.id, second.id),
    };
    Some(format!(
        "model_too_large: {} needs ~{} {unit}; this device budgets {} {unit} — choose {choices}",
        model.id,
        thousands(model.baseline_mb),
        thousands(limits.budget_mb),
        unit = MODEL_BASE_UNIT,
    ))
}

// ---- platform providers ---------------------------------------------------

/// Reads this host's memory report, or names why it cannot.
pub fn probe_host() -> Result<MemoryProbe, String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        read_meminfo_at(Path::new("/proc/meminfo"))
    }
    #[cfg(target_os = "macos")]
    {
        macos_probe()
    }
    #[cfg(windows)]
    {
        windows_probe()
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        windows
    )))]
    {
        Err(format!(
            "no memory probe for this platform ({})",
            Platform::host().as_str()
        ))
    }
}

/// Parses `/proc/meminfo` into `(MemTotal, MemAvailable)` in bytes.
///
/// `MemAvailable` is the honest ceiling; an older kernel without it falls back
/// to `MemTotal`, which is what the design specifies. A file with no
/// `MemTotal` is not guessed at: `None`.
#[cfg(any(target_os = "linux", target_os = "android", test))]
pub fn parse_meminfo(text: &str) -> Option<(u64, Option<u64>)> {
    let mut total: Option<u64> = None;
    let mut available: Option<u64> = None;
    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = rest.split_whitespace().next() else {
            continue;
        };
        let Ok(kilobytes) = value.parse::<u64>() else {
            continue;
        };
        match key.trim() {
            "MemTotal" => total = Some(kilobytes.saturating_mul(1024)),
            "MemAvailable" => available = Some(kilobytes.saturating_mul(1024)),
            _ => {}
        }
    }
    Some((total?, available))
}

/// Reads a `/proc/meminfo`-shaped file. The path is a parameter so the failing
/// provider is pinned by a test.
#[cfg(any(target_os = "linux", target_os = "android", test))]
pub fn read_meminfo_at(path: &Path) -> Result<MemoryProbe, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let (total, available) =
        parse_meminfo(&text).ok_or_else(|| format!("{}: no MemTotal line", path.display()))?;
    // The design's fallback: a kernel without MemAvailable reports MemTotal as
    // the available figure (conservative relative to a real MemAvailable).
    let available_bytes = Some(available.unwrap_or(total));
    Ok(MemoryProbe {
        total_bytes: total,
        available_bytes,
        low_ram: false,
    })
}

/// macOS: `sysctlbyname("hw.memsize")` — the installed physical memory.
#[cfg(target_os = "macos")]
fn macos_probe() -> Result<MemoryProbe, String> {
    let name = std::ffi::CString::new("hw.memsize").expect("a literal without NUL");
    let mut value: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    // SAFETY: `name` is a valid NUL-terminated C string; `value` is a properly
    // aligned u64 and `size` its exact byte length, which is what
    // `sysctlbyname` writes into.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut value as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(format!(
            "sysctlbyname(hw.memsize): {}",
            std::io::Error::last_os_error()
        ));
    }
    if value == 0 {
        return Err("sysctlbyname(hw.memsize) returned 0 bytes".to_string());
    }
    Ok(MemoryProbe {
        total_bytes: value,
        available_bytes: None,
        low_ram: false,
    })
}

/// Windows: `GlobalMemoryStatusEx` (15 lines of FFI, no crate).
#[cfg(windows)]
fn windows_probe() -> Result<MemoryProbe, String> {
    windows_memory::probe()
}

#[cfg(windows)]
mod windows_memory {
    #[repr(C)]
    #[allow(non_snake_case)]
    struct MemoryStatusEx {
        dwLength: u32,
        dwMemoryLoad: u32,
        ullTotalPhys: u64,
        ullAvailPhys: u64,
        ullTotalPageFile: u64,
        ullAvailPageFile: u64,
        ullTotalVirtual: u64,
        ullAvailVirtual: u64,
        ullAvailExtendedVirtual: u64,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    pub fn probe() -> Result<super::MemoryProbe, String> {
        let mut status = MemoryStatusEx {
            dwLength: std::mem::size_of::<MemoryStatusEx>() as u32,
            dwMemoryLoad: 0,
            ullTotalPhys: 0,
            ullAvailPhys: 0,
            ullTotalPageFile: 0,
            ullAvailPageFile: 0,
            ullTotalVirtual: 0,
            ullAvailVirtual: 0,
            ullAvailExtendedVirtual: 0,
        };
        // SAFETY: `status` is a zeroed, correctly sized `MEMORYSTATUSEX` with
        // `dwLength` set to its own size, which is the API's contract.
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        if ok == 0 {
            return Err(format!(
                "GlobalMemoryStatusEx: {}",
                std::io::Error::last_os_error()
            ));
        }
        if status.ullTotalPhys == 0 {
            return Err("GlobalMemoryStatusEx returned 0 bytes of RAM".to_string());
        }
        // The design's Windows bound is the commit limit: total physical plus
        // what the pagefile can still commit.
        Ok(super::MemoryProbe {
            total_bytes: status.ullTotalPhys,
            available_bytes: Some(status.ullAvailPageFile),
            low_ram: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    const GB: u64 = 1_000_000_000;

    fn footprint(id: &str) -> &'static ModelFootprint {
        footprint_for(id).expect("a table model")
    }

    struct Case {
        name: &'static str,
        platform: Platform,
        total: u64,
        available: Option<u64>,
        low_ram: bool,
        budget: u32,
        minutes: [u32; 2],
        fits: [bool; 2],
    }

    /// The design's worked table, exactly: {4 GB low-RAM, 4 GB, 12 GB Vivo,
    /// 16 GB Mac, 64 GB Mac} × {tiny, turbo-q5-0}. Budgets and caps are the
    /// formula's output, not the constants the slice replaces.
    #[test]
    fn the_design_device_table_is_exact() {
        // 64 GiB of free disk: the disk rule must not bind in this table.
        let disk = Some(64 * GIB);
        // The design's worked table assumes the total-only budget (available
        // not wired), which is exactly what its arithmetic shows:
        // `min(5040, 9200)` on the 12 GB phone. Production with `availMem`
        // wired takes the tighter `avail * 0.65` bound.
        let cases = [
            Case {
                name: "4 GB low-RAM",
                platform: Platform::Android,
                total: 4 * GB,
                available: None,
                low_ram: true,
                budget: 850,
                minutes: [58, 0],
                fits: [true, false],
            },
            Case {
                name: "4 GB",
                platform: Platform::Android,
                total: 4 * GB,
                available: None,
                low_ram: false,
                budget: 1_020,
                minutes: [77, 0],
                fits: [true, false],
            },
            Case {
                name: "12 GB Vivo",
                platform: Platform::Android,
                total: 12 * GB,
                available: None,
                low_ram: false,
                budget: 4_284,
                minutes: [431, 309],
                fits: [true, true],
            },
            // 16 GiB Mac: (total - 4,096 MB) * 0.60 * 0.85.
            Case {
                name: "16 GB Mac",
                platform: Platform::MacOs,
                total: 16 * GIB,
                available: None,
                low_ram: false,
                budget: 6_672,
                minutes: [480, 480],
                fits: [true, true],
            },
            // 64 GiB Mac: the 8-hour ceiling binds first.
            Case {
                name: "64 GB Mac",
                platform: Platform::MacOs,
                total: 64 * GIB,
                available: None,
                low_ram: false,
                budget: 32_957,
                minutes: [480, 480],
                fits: [true, true],
            },
        ];

        println!("| device | total | model | budget MB | max_minutes | model_fits | reason |");
        for case in cases {
            let Case {
                name,
                platform,
                total,
                available,
                low_ram,
                budget,
                minutes,
                fits,
            } = case;
            for (index, model) in ["tiny", "turbo-q5-0"].iter().enumerate() {
                let model = footprint(model);
                let limits = compute_device_limits(
                    platform,
                    total,
                    available,
                    low_ram,
                    model.baseline_mb,
                    model.slope_mb_per_min,
                    disk,
                );
                println!(
                    "| {name} | {} MB | {} | {} | {} | {} | {} |",
                    total / 1_000_000,
                    model.id,
                    limits.budget_mb,
                    limits.max_minutes,
                    limits.model_fits,
                    limits.reason.as_str()
                );
                assert_eq!(limits.budget_mb, budget, "{name} / {}: budget", model.id);
                assert_eq!(
                    limits.max_minutes, minutes[index],
                    "{name} / {}: max_minutes",
                    model.id
                );
                assert_eq!(
                    limits.model_fits, fits[index],
                    "{name} / {}: model_fits",
                    model.id
                );
            }
        }
        // The refusal copy carries the design's numbers verbatim.
        let phone = compute_device_limits(
            Platform::Android,
            4 * GB,
            Some(2 * GB),
            false,
            310,
            9.2,
            Some(64 * GIB),
        );
        assert_eq!(
            phone.budget_line(footprint("tiny")),
            "this device budgets 1,020 MB; tiny needs 310 MB + 9.2 MB per minute"
        );
        let vivo = compute_device_limits(Platform::Android, 12 * GB, None, false, 310, 9.2, disk);
        assert_eq!(
            vivo.budget_line(footprint("tiny")),
            "this device budgets 4,284 MB; tiny needs 310 MB + 9.2 MB per minute"
        );
    }

    /// The model-fit refusal names the numbers and the fix — no downgrade.
    #[test]
    fn the_model_fit_refusal_names_the_too_large_model_and_the_fitting_ones() {
        let phone = compute_device_limits(
            Platform::Android,
            4 * GB,
            Some(2 * GB),
            false,
            1_039,
            10.5,
            Some(64 * GIB),
        );
        let refusal =
            model_fit_refusal(footprint("turbo-q5-0"), &phone).expect("turbo-q5-0 is over 1,020");
        assert_eq!(
            refusal,
            "model_too_large: turbo-q5-0 needs ~1,039 MB; this device budgets 1,020 MB — \
             choose small or base"
        );
        // The model that fits produces no refusal at all.
        let small_limits = compute_device_limits(
            Platform::Android,
            4 * GB,
            Some(2 * GB),
            false,
            700,
            9.2,
            Some(64 * GIB),
        );
        assert!(model_fit_refusal(footprint("small"), &small_limits).is_none());
        let tiny_limits = compute_device_limits(
            Platform::Android,
            4 * GB,
            Some(2 * GB),
            false,
            310,
            9.2,
            Some(64 * GIB),
        );
        assert!(model_fit_refusal(footprint("tiny"), &tiny_limits).is_none());
    }

    /// The conservative recording-cap model: the largest row that fits.
    #[test]
    fn the_largest_fitting_model_is_the_conservative_recording_cap_model() {
        assert_eq!(largest_fitting(1_020).map(|model| model.id), Some("small"));
        assert_eq!(
            largest_fitting(1_596).map(|model| model.id),
            Some("turbo-q5-0")
        );
        assert_eq!(largest_fitting(448).map(|model| model.id), Some("tiny"));
        assert_eq!(largest_fitting(100), None);
    }

    /// The Linux provider against a fixture string — both with and without
    /// MemAvailable.
    #[test]
    fn the_linux_provider_parses_a_meminfo_fixture() {
        let fixture = "\
MemTotal:       16384000 kB
MemFree:         2000000 kB
MemAvailable:    8192000 kB
Buffers:          100000 kB
Cached:          3000000 kB
SwapCached:            0 kB
";
        let (total, available) = parse_meminfo(fixture).expect("a MemTotal line");
        assert_eq!(total, 16_384_000 * 1024);
        assert_eq!(available, Some(8_192_000 * 1024));

        // An older kernel with no MemAvailable falls back to MemTotal.
        let old = "MemTotal: 4000000 kB\nMemFree: 1000000 kB\n";
        let (total, available) = parse_meminfo(old).expect("a MemTotal line");
        assert_eq!(total, 4_000_000 * 1024);
        assert_eq!(available, None);

        // No MemTotal is not guessed at.
        assert_eq!(parse_meminfo("MemFree: 1 kB\n"), None);
        assert_eq!(parse_meminfo(""), None);
    }

    /// The failing provider: a named error, the documented fallback, and a log
    /// line that names the cause.
    #[test]
    fn a_failing_provider_yields_the_documented_fallback_and_a_log_line() {
        #[derive(Default)]
        struct Capture {
            events: Arc<Mutex<Vec<String>>>,
        }

        struct Visitor(String);

        impl tracing::field::Visit for Visitor {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if !self.0.is_empty() {
                    self.0.push(' ');
                }
                self.0.push_str(&format!("{}={:?}", field.name(), value));
            }
        }

        impl tracing::Subscriber for Capture {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                let mut visitor = Visitor(String::new());
                event.record(&mut visitor);
                self.events.lock().unwrap().push(visitor.0);
            }
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        let capture = Capture::default();
        let events = capture.events.clone();
        let limits = tracing::subscriber::with_default(capture, || {
            limits_from_probe(
                Err("/proc/meminfo: No such file or directory".to_string()),
                Platform::Linux,
                footprint("tiny"),
                Some(64 * GIB),
            )
        });

        assert_eq!(limits, fallback_limits());
        assert_eq!(limits.max_minutes, 60);
        assert_eq!(limits.reason, CapReason::Fallback);
        assert!(
            limits.model_fits,
            "the fallback cannot prove a model too large"
        );
        assert_eq!(limits.budget_mb, 0);
        assert!(limits
            .budget_line(footprint("tiny"))
            .contains("conservative 60-minute cap"));

        let logged = events.lock().unwrap();
        assert_eq!(logged.len(), 1, "exactly one warning: {logged:?}");
        assert!(
            logged[0].contains("device memory probe failed"),
            "{}",
            logged[0]
        );
        assert!(
            logged[0].contains("/proc/meminfo: No such file or directory"),
            "the log names the failure: {}",
            logged[0]
        );
        assert!(logged[0].contains("platform=\"linux\""), "{}", logged[0]);
    }

    /// A missing fixture path is the provider's own named failure.
    #[test]
    fn a_missing_meminfo_file_names_its_path() {
        let missing = std::env::temp_dir().join("suramak-m26a-no-such-meminfo");
        let _ = std::fs::remove_file(&missing);
        let error = read_meminfo_at(&missing).expect_err("a missing file is an error");
        assert!(error.contains("suramak-m26a-no-such-meminfo"), "{error}");
    }

    /// The disk rule is the design's `(free - 500 MB) / 3.84`; a short disk
    /// lowers the cap and says `disk`.
    #[test]
    fn free_disk_can_be_the_binding_constraint() {
        let limits = compute_device_limits(
            Platform::Linux,
            32 * GIB,
            Some(16 * GIB),
            false,
            310,
            9.2,
            Some(1_000_000_000),
        );
        // (1,000,000,000 - 500,000,000) / 3,840,000 = 130.2 → 130.
        assert_eq!(limits.max_minutes, 130);
        assert_eq!(limits.reason, CapReason::Disk);
    }

    /// The eight-hour ceiling clamps a generous desktop and says so.
    #[test]
    fn the_ceiling_is_eight_hours_and_is_named() {
        let limits = compute_device_limits(
            Platform::MacOs,
            64 * GIB,
            None,
            false,
            310,
            9.2,
            Some(64 * GIB),
        );
        assert_eq!(limits.max_minutes, MAX_MINUTES_CEILING);
        assert_eq!(limits.reason, CapReason::Ceiling);
    }

    /// The model table is one table, and its rows are the design's.
    #[test]
    fn the_model_table_is_the_designs_five_rows() {
        let rows: Vec<(&str, u32, f32)> = MODEL_FOOTPRINTS
            .iter()
            .map(|model| (model.id, model.baseline_mb, model.slope_mb_per_min))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("tiny", 310, 9.2),
                ("base", 380, 9.2),
                ("small", 700, 9.2),
                ("turbo-q5-0", 1_039, 10.5),
                ("turbo", 1_900, 10.5),
            ]
        );
        // A file path resolves to the same row; an unknown file is not gated.
        assert_eq!(
            footprint_for_path(Path::new("/models/ggml-turbo-q5-0.bin")).map(|m| m.id),
            Some("turbo-q5-0")
        );
        assert_eq!(footprint_for_path(Path::new("/tmp/custom.bin")), None);
    }

    /// The live provider: every supported host must answer (Linux from
    /// `/proc/meminfo`), and the cap it produces is in the documented range.
    #[test]
    fn the_live_probe_answers_and_the_cap_is_computed() {
        let probe = probe_host().expect("the host memory probe must answer");
        assert!(probe.total_bytes > 0);
        if cfg!(any(target_os = "linux", target_os = "android")) {
            assert!(
                probe.available_bytes.is_some_and(|available| available > 0),
                "MemAvailable (or its MemTotal fallback) must be present: {probe:?}"
            );
            assert!(
                probe.available_bytes.unwrap() <= probe.total_bytes,
                "available cannot exceed total: {probe:?}"
            );
        }
        let disk = host_free_disk_bytes();
        let limits = limits_for_model(footprint("tiny"), disk);
        println!(
            "M26A_HOST platform={} total_bytes={} available_bytes={:?} low_ram={} \
             free_disk_bytes={disk:?} budget_mb={} max_minutes={} model_fits={} reason={}",
            Platform::host().as_str(),
            probe.total_bytes,
            probe.available_bytes,
            probe.low_ram,
            limits.budget_mb,
            limits.max_minutes,
            limits.model_fits,
            limits.reason.as_str(),
        );
        assert!(
            (1..=MAX_MINUTES_CEILING).contains(&limits.max_minutes),
            "{limits:?}"
        );
    }

    /// A probe with no bytes is the fallback, not a zero budget.
    #[test]
    fn a_zero_byte_probe_is_the_fallback() {
        let limits = compute_device_limits(Platform::Linux, 0, None, false, 310, 9.2, None);
        assert_eq!(limits, fallback_limits());
    }

    /// Thousands separators in the copy are the design's (`4,284`).
    #[test]
    fn the_copy_formats_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(310), "310");
        assert_eq!(thousands(1_020), "1,020");
        assert_eq!(thousands(4_284), "4,284");
        assert_eq!(thousands(32_957), "32,957");
    }
}
