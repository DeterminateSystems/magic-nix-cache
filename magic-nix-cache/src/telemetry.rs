use std::env;
use std::time::SystemTime;

use sha2::{Digest, Sha256};

/// A telemetry report to measure the effectiveness of the Magic Nix Cache
#[derive(Debug, Default, serde::Serialize)]
pub struct TelemetryReport {
    distinct_id: Option<String>,

    version: String,
    is_ci: bool,

    #[serde(skip_serializing)]
    start_time: Option<SystemTime>,
    elapsed_seconds: Metric,

    pub narinfos_served: Metric,
    pub narinfos_sent_upstream: Metric,
    pub narinfos_negative_cache_hits: Metric,
    pub narinfos_negative_cache_misses: Metric,
    pub narinfos_uploaded: Metric,

    pub nars_served: Metric,
    pub nars_sent_upstream: Metric,
    pub nars_uploaded: Metric,

    pub num_original_paths: Metric,
    pub num_final_paths: Metric,
    pub num_new_paths: Metric,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct Metric(std::sync::atomic::AtomicUsize);
impl Metric {
    pub fn incr(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    pub fn set(&self, val: usize) {
        self.0.store(val, std::sync::atomic::Ordering::Relaxed);
    }
}

impl TelemetryReport {
    pub fn new() -> TelemetryReport {
        TelemetryReport {
            distinct_id: calculate_opaque_id().ok(),

            version: env!("CARGO_PKG_VERSION").to_string(),
            is_ci: is_ci::cached(),

            start_time: Some(SystemTime::now()),

            ..Default::default()
        }
    }

    pub async fn send(&self, endpoint: &str) {
        if let Some(start_time) = self.start_time {
            self.elapsed_seconds.set(
                SystemTime::now()
                    .duration_since(start_time)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(usize::MAX),
            );
        }

        if let Ok(serialized) = serde_json::to_string_pretty(&self) {
            let _ = reqwest::Client::new()
                .post(endpoint)
                .body(serialized)
                .header("Content-Type", "application/json")
                .timeout(std::time::Duration::from_millis(3000))
                .send()
                .await;
        }
    }
}

fn calculate_opaque_id() -> Result<String, env::VarError> {
    let mut hasher = Sha256::new();
    hasher.update(env::var("GITHUB_REPOSITORY")?);
    hasher.update(env::var("GITHUB_REPOSITORY_ID")?);
    hasher.update(env::var("GITHUB_REPOSITORY_OWNER")?);
    hasher.update(env::var("GITHUB_REPOSITORY_OWNER_ID")?);

    let result = hasher.finalize();
    Ok(format!("{:x}", result))
}
