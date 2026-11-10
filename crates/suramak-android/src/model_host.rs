//! M13d: the model-download host behind the Android `JobService`.
//!
//! Kotlin owns the job, the notification and the cancel button; Rust owns the
//! transfer, the checksum and the resume bookmark. The two meet in one
//! JSON-in/JSON-out command ([`ModelHost::dispatch`]) rather than one JNI
//! function per feature — the same shape the M13c plan asked for.
//!
//! Why a JSON command and not typed JNI:
//!
//! * the whole host is host-testable without an emulator ([`ModelHost::new`]
//!   instances are independent; the JNI export only wraps a process-wide one);
//! * the Kotlin side already has `org.json`, so no dependency is added there;
//! * a new operation is one `match` arm, not a new JNI symbol to get wrong.
//!
//! Operations (all requests carry `"op"`):
//!
//! | op | request fields | reply |
//! |---|---|---|
//! | `memory` | `total_ram_bytes`, `available_ram_bytes`, `low_memory`, `arm64`, `language` | the device tier + recommendation |
//! | `plan` | `model`, `language` | whether this device may download `model` |
//! | `start` | `model`, `language`, `models_dir` | starts the install (refuses before any byte) |
//! | `progress` | — | phase, bytes, deterministic percent, message |
//! | `cancel` | — | asks the worker to stop; the part stays |
//! | `space` | `models_dir`, `needed` | free-space preflight result |
//! | `landing` | `language` | the landing copy (EN / ID) |
//! | `version` | — | crate version |
//!
//! **Honest boundary:** the download itself is
//! [`suramak_core::download::install_file`], whose resume/checksum behaviour is
//! unit-tested in `suramak-core`; this module adds the process state, the
//! refusals and the progress mapping, and is tested on the host. The JNI symbol
//! itself is compile-checked for `aarch64-linux-android`; no emulator runs in
//! this slice (WORKORDER-M13D).

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use suramak_core::device::{self, DeviceProfile, PhoneTier};
use suramak_core::download;
use suramak_core::models;
use suramak_core::SuramakError;

/// The lifecycle of one download attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Idle => "idle",
            Phase::Running => "running",
            Phase::Done => "done",
            Phase::Failed => "failed",
            Phase::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug)]
struct JobState {
    phase: Phase,
    model: String,
    bytes_done: u64,
    bytes_total: u64,
    message: String,
}

impl JobState {
    fn idle() -> Self {
        Self {
            phase: Phase::Idle,
            model: String::new(),
            bytes_done: 0,
            bytes_total: 0,
            message: String::new(),
        }
    }
}

struct Inner {
    profile: Mutex<Option<DeviceProfile>>,
    state: Mutex<JobState>,
    cancel: AtomicBool,
}

/// One download host. Cheap to clone (the state is shared); tests create their
/// own instances, the JNI export uses the process-wide [`crate::host`].
#[derive(Clone)]
pub struct ModelHost {
    inner: Arc<Inner>,
}

impl Default for ModelHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelHost {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Inner {
                profile: Mutex::new(None),
                state: Mutex::new(JobState::idle()),
                cancel: AtomicBool::new(false),
            }),
        }
    }

    /// Runs one command; always returns a JSON object, never panics on bad input.
    pub fn dispatch(&self, request: &str) -> String {
        let Ok(value) = serde_json::from_str::<Value>(request) else {
            return json!({ "ok": false, "error": "invalid JSON request" }).to_string();
        };
        let op = value.get("op").and_then(Value::as_str).unwrap_or_default();
        let reply = match op {
            "memory" => self.memory(&value),
            "plan" => self.plan(&value),
            "start" => self.start(&value),
            "progress" => self.progress(),
            "cancel" => self.cancel(),
            "space" => self.space(&value),
            "landing" => {
                let language = value
                    .get("language")
                    .and_then(Value::as_str)
                    .unwrap_or("en");
                json!({ "ok": true, "text": device::landing_text(language) })
            }
            "version" => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }),
            other => json!({ "ok": false, "error": format!("unknown op '{other}'") }),
        };
        reply.to_string()
    }

    /// Records the platform memory report and returns the recommendation.
    fn memory(&self, request: &Value) -> Value {
        let profile = DeviceProfile::new(
            request
                .get("total_ram_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            request
                .get("available_ram_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            request
                .get("low_memory")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            request
                .get("arm64")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        );
        *self.inner.profile.lock().expect("profile lock") = Some(profile);
        let language = request
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("en");
        recommendation_json(profile, language)
    }

    fn profile(&self) -> Option<DeviceProfile> {
        *self.inner.profile.lock().expect("profile lock")
    }

    fn plan(&self, request: &Value) -> Value {
        let Some(profile) = self.profile() else {
            return json!({
                "ok": true,
                "allowed": false,
                "reason": "device memory has not been probed yet",
            });
        };
        let model = request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let language = request
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("en");
        if models::find_model(model).is_none() {
            return json!({ "ok": false, "error": format!("unknown model '{model}'") });
        }
        let reason = device::refusal_for(profile, language, model);
        let recommendation = device::recommend(profile, language);
        json!({
            "ok": true,
            "model": model,
            "allowed": reason.is_none(),
            "reason": reason,
            "default_model": recommendation.default_model_id,
        })
    }

    fn start(&self, request: &Value) -> Value {
        let Some(profile) = self.profile() else {
            return json!({
                "ok": false,
                "refused": "device memory has not been probed yet — start the job from the app",
            });
        };
        let model_id = request
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let language = request
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("en");
        let models_dir = request
            .get("models_dir")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if models_dir.is_empty() {
            return json!({ "ok": false, "refused": "no models directory was provided" });
        }
        let Some(info) = models::find_model(model_id) else {
            return json!({ "ok": false, "refused": format!("unknown model '{model_id}'") });
        };
        if let Some(reason) = device::refusal_for(profile, language, model_id) {
            return json!({ "ok": false, "refused": reason });
        }
        // The free-space preflight runs here, before the worker exists, so a
        // refusal leaves no `.part` and no thread behind.
        if let Err(err) = download::check_space(Path::new(models_dir), info.size_bytes) {
            return json!({ "ok": false, "refused": err.to_string() });
        }

        {
            let mut state = self.inner.state.lock().expect("state lock");
            if state.phase == Phase::Running {
                return json!({
                    "ok": false,
                    "refused": "a model download is already running",
                });
            }
            *state = JobState {
                phase: Phase::Running,
                model: model_id.to_string(),
                bytes_done: 0,
                bytes_total: info.size_bytes,
                message: "starting".to_string(),
            };
        }
        self.inner.cancel.store(false, Ordering::SeqCst);

        let inner = self.inner.clone();
        let url = models::model_url(info);
        let file_name = info.hf_file;
        let size = info.size_bytes;
        let sha256 = info.sha256;
        let dir = models_dir.to_string();
        std::thread::spawn(move || {
            let result =
                download::install_file(Path::new(&dir), file_name, &url, size, sha256, |written| {
                    {
                        let mut state = inner.state.lock().expect("state lock");
                        state.bytes_done = written.min(size);
                    }
                    !inner.cancel.load(Ordering::SeqCst)
                });
            let mut state = inner.state.lock().expect("state lock");
            match result {
                Ok(()) => {
                    state.phase = Phase::Done;
                    state.bytes_done = size;
                    state.message = "download complete, sha256 verified".to_string();
                }
                Err(SuramakError::Cancelled) => {
                    state.phase = Phase::Cancelled;
                    state.message =
                        "paused — the partial file is kept and resumes next time".to_string();
                }
                Err(err) => {
                    state.phase = Phase::Failed;
                    state.message = err.to_string();
                }
            }
        });

        json!({
            "ok": true,
            "state": "running",
            "model": model_id,
            "bytes_total": info.size_bytes,
        })
    }

    fn progress(&self) -> Value {
        let state = self.inner.state.lock().expect("state lock");
        json!({
            "ok": true,
            "state": state.phase.as_str(),
            "model": state.model,
            "bytes_done": state.bytes_done,
            "bytes_total": state.bytes_total,
            "percent": percent(state.bytes_done, state.bytes_total),
            "message": state.message,
        })
    }

    fn cancel(&self) -> Value {
        let running = self.inner.state.lock().expect("state lock").phase == Phase::Running;
        if running {
            self.inner.cancel.store(true, Ordering::SeqCst);
        }
        json!({ "ok": true, "cancel_requested": running })
    }

    fn space(&self, request: &Value) -> Value {
        let models_dir = request
            .get("models_dir")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let needed = request.get("needed").and_then(Value::as_u64).unwrap_or(0);
        if models_dir.is_empty() {
            return json!({ "ok": false, "error": "no models directory was provided" });
        }
        match download::check_space(Path::new(models_dir), needed) {
            Ok(()) => json!({
                "ok": true,
                "enough": true,
                "needed": needed,
                "available": download::available_space_bytes(Path::new(models_dir)),
                "message": "",
            }),
            Err(SuramakError::NotEnoughSpace {
                needed,
                available,
                message,
                ..
            }) => json!({
                "ok": true,
                "enough": false,
                "needed": needed,
                "available": available,
                "message": message,
            }),
            Err(err) => json!({ "ok": false, "error": err.to_string() }),
        }
    }
}

/// Integer percent — deterministic for a given byte pair (the notification
/// never shows a value that a re-read would contradict).
pub fn percent(bytes_done: u64, bytes_total: u64) -> u64 {
    if bytes_total == 0 {
        return 0;
    }
    (bytes_done.min(bytes_total) * 100) / bytes_total
}

fn tier_str(tier: PhoneTier) -> &'static str {
    match tier {
        PhoneTier::Unsupported => "unsupported",
        PhoneTier::BelowFloor => "below_floor",
        PhoneTier::Low => "low",
        PhoneTier::Mid => "mid",
        PhoneTier::High => "high",
    }
}

fn recommendation_json(profile: DeviceProfile, language: &str) -> Value {
    let recommendation = device::recommend(profile, language);
    let refused: Vec<Value> = recommendation
        .refused
        .iter()
        .map(|refusal| json!({ "id": refusal.id, "reason": refusal.reason }))
        .collect();
    json!({
        "ok": true,
        "tier": tier_str(recommendation.tier),
        "tier_label": recommendation.tier.label(),
        "meets_floor": recommendation.tier.meets_floor(),
        "total_ram_bytes": profile.total_ram_bytes,
        "ram_gb": profile.ram_gb(),
        "default_model": recommendation.default_model_id,
        "allowed": recommendation.allowed,
        "refused": refused,
        "headline": recommendation.headline,
        "detail": recommendation.detail,
        "language_note": recommendation.language_note,
        "landing": device::landing_text(language),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const GB: u64 = 1_000_000_000;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("suramak-host-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn request(host: &ModelHost, value: Value) -> Value {
        serde_json::from_str(&host.dispatch(&value.to_string())).expect("a JSON reply")
    }

    fn probe(host: &ModelHost, ram_gb: f64, language: &str) -> Value {
        request(
            host,
            json!({
                "op": "memory",
                "total_ram_bytes": (ram_gb * GB as f64) as u64,
                "available_ram_bytes": GB,
                "low_memory": false,
                "arm64": true,
                "language": language,
            }),
        )
    }

    /// 4/6/8 GB: base, base, small — and the reply says why.
    #[test]
    fn memory_replies_with_the_tier_default_and_the_landing_copy() {
        let host = ModelHost::new();
        for (ram, tier, default) in [
            (3.7, "low", "base"),
            (5.5, "mid", "base"),
            (7.4, "high", "small"),
        ] {
            let reply = probe(&host, ram, "en");
            assert_eq!(reply["ok"], true);
            assert_eq!(reply["tier"], tier, "{ram} GB: {reply}");
            assert_eq!(reply["default_model"], default, "{ram} GB: {reply}");
            assert_eq!(reply["meets_floor"], true);
            assert!(reply["landing"].as_str().unwrap().contains("base"));
            assert!(reply["allowed"]
                .as_array()
                .unwrap()
                .contains(&json!("tiny")));
        }
    }

    /// Below the floor: only tiny, everything larger refused with the reason.
    #[test]
    fn below_the_floor_reply_is_explicit() {
        let host = ModelHost::new();
        let reply = probe(&host, 2.7, "en");
        assert_eq!(reply["tier"], "below_floor");
        assert_eq!(reply["meets_floor"], false);
        assert_eq!(reply["default_model"], "tiny");
        let refused = reply["refused"].as_array().unwrap();
        assert!(refused.iter().any(
            |entry| entry["id"] == "base" && entry["reason"].as_str().unwrap().contains("4 GB")
        ));
    }

    /// Indonesian carries the tiny warning in the reply.
    #[test]
    fn indonesian_reply_carries_the_tiny_warning() {
        let host = ModelHost::new();
        let reply = probe(&host, 7.4, "id-ID");
        assert!(reply["language_note"].as_str().unwrap().contains("tiny"));
        assert!(reply["landing"].as_str().unwrap().contains("Indonesia"));
    }

    /// A download started before the memory probe is refused, not guessed.
    #[test]
    fn start_before_the_memory_probe_is_refused() {
        let host = ModelHost::new();
        let dir = scratch("noprobe");
        let reply = request(
            &host,
            json!({ "op": "start", "model": "base", "language": "en", "models_dir": dir.to_string_lossy() }),
        );
        assert_eq!(reply["ok"], false);
        assert!(reply["refused"].as_str().unwrap().contains("probed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Below the floor, `start` refuses `small` before creating anything.
    #[test]
    fn start_refuses_a_model_this_phone_cannot_run() {
        let host = ModelHost::new();
        probe(&host, 2.7, "en");
        let dir = scratch("refuse-small");
        let reply = request(
            &host,
            json!({ "op": "start", "model": "small", "language": "en", "models_dir": dir.to_string_lossy() }),
        );
        assert_eq!(reply["ok"], false, "{reply}");
        assert!(
            reply["refused"].as_str().unwrap().contains("4 GB"),
            "{reply}"
        );
        assert!(
            !dir.join("ggml-small.bin.part").exists(),
            "a refusal must not create a partial file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Models never offered on phones are refused with their own reason.
    #[test]
    fn start_refuses_a_model_that_is_never_offered_on_phones() {
        let host = ModelHost::new();
        probe(&host, 7.4, "en");
        let dir = scratch("refuse-turbo");
        let reply = request(
            &host,
            json!({ "op": "start", "model": "turbo", "language": "en", "models_dir": dir.to_string_lossy() }),
        );
        assert_eq!(reply["ok"], false);
        assert!(reply["refused"]
            .as_str()
            .unwrap()
            .contains("not offered on phones"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The free-space preflight is reachable from Kotlin and reports both
    /// numbers; it refuses an impossible requirement without starting work.
    #[test]
    fn space_reports_a_refusal_with_the_numbers() {
        let host = ModelHost::new();
        let dir = scratch("space");
        let reply = request(
            &host,
            json!({ "op": "space", "models_dir": dir.to_string_lossy(), "needed": u64::MAX }),
        );
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["enough"], false);
        assert!(reply["message"].as_str().unwrap().contains("free up space"));
        assert!(reply["needed"].as_u64().unwrap() > 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Progress starts idle and the percent is integer/deterministic.
    #[test]
    fn progress_is_idle_before_a_download_and_percent_is_deterministic() {
        let host = ModelHost::new();
        let reply = request(&host, json!({ "op": "progress" }));
        assert_eq!(reply["state"], "idle");
        assert_eq!(reply["percent"], 0);
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(50, 200), 25);
        assert_eq!(percent(199, 200), 99);
        assert_eq!(percent(500, 200), 100, "never past 100");
    }

    /// Unknown models, bad JSON and unknown ops fail closed.
    #[test]
    fn bad_requests_fail_closed() {
        let host = ModelHost::new();
        let bad_json: Value = serde_json::from_str(&host.dispatch("{not json")).unwrap();
        assert_eq!(bad_json["ok"], false);
        let unknown_op: Value = serde_json::from_str(&host.dispatch(r#"{"op":"nope"}"#)).unwrap();
        assert_eq!(unknown_op["ok"], false);
        probe(&host, 7.4, "en");
        let unknown_model = request(
            &host,
            json!({ "op": "start", "model": "huge", "language": "en", "models_dir": "/tmp" }),
        );
        assert_eq!(unknown_model["ok"], false);
        assert!(unknown_model["refused"]
            .as_str()
            .unwrap()
            .contains("unknown model"));
    }

    /// `cancel` on an idle host is a no-op, not an error.
    #[test]
    fn cancel_on_an_idle_host_reports_nothing_to_cancel() {
        let host = ModelHost::new();
        let reply = request(&host, json!({ "op": "cancel" }));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["cancel_requested"], false);
    }
}
