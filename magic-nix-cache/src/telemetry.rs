use std::time::SystemTime;

use detsys_ids_client::Recorder;

/// A telemetry report to measure the effectiveness of the Magic Nix Cache
#[derive(Debug, Default)]
pub struct TelemetryReport {
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

    pub tripped_429: std::sync::atomic::AtomicBool,
    recorder: Option<Recorder>,
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

macro_rules! fact {
    ($recorder:ident, $property:ident) => {{
        if let Ok(prop) = serde_json::to_value($property) {
            $recorder.set_fact(stringify!($property), prop).await;
        }
    }};
}

impl TelemetryReport {
    pub fn new(recorder: Recorder) -> TelemetryReport {
        TelemetryReport {
            recorder: Some(recorder),
            start_time: Some(SystemTime::now()),

            ..Default::default()
        }
    }

    pub async fn send(&self) {
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

        let TelemetryReport {
            start_time: _,
            elapsed_seconds,
            narinfos_served,
            narinfos_sent_upstream,
            narinfos_negative_cache_hits,
            narinfos_negative_cache_misses,
            narinfos_uploaded,
            nars_served,
            nars_sent_upstream,
            nars_uploaded,
            num_original_paths,
            num_final_paths,
            num_new_paths,
            tripped_429,
            recorder,
        } = self;

        let Some(recorder) = recorder else {
            return;
        };

        fact!(recorder, elapsed_seconds);
        fact!(recorder, narinfos_served);
        fact!(recorder, narinfos_sent_upstream);
        fact!(recorder, narinfos_negative_cache_hits);
        fact!(recorder, narinfos_negative_cache_misses);
        fact!(recorder, narinfos_uploaded);
        fact!(recorder, nars_served);
        fact!(recorder, nars_sent_upstream);
        fact!(recorder, nars_uploaded);
        fact!(recorder, num_original_paths);
        fact!(recorder, num_final_paths);
        fact!(recorder, num_new_paths);
        fact!(recorder, tripped_429);
    }
}
