//! Prometheus metrics: registry + 12 typed helpers.

use std::sync::OnceLock;

use prometheus::{HistogramOpts, HistogramVec, IntCounterVec, IntGauge, Opts, Registry};

static REGISTRY: OnceLock<Registry> = OnceLock::new();

pub fn global_registry() -> &'static Registry {
    REGISTRY.get_or_init(Registry::new)
}

// --- Buckets: 30us-1s (README baseline is 30us at 50k TPS) ---
const LATENCY_BUCKETS: &[f64] = &[0.0001, 0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0];

// Each macro expands at the call site, so every metric helper owns a
// dedicated `STORAGE` static. (A plain `fn` with a `static STORAGE`
// inside would share storage across all call sites of the function.)
macro_rules! lazy_counter_vec {
    ($name:expr, $help:expr, $labels:expr $(,)?) => {{
        static STORAGE: OnceLock<IntCounterVec> = OnceLock::new();
        STORAGE.get_or_init(|| {
            let v = IntCounterVec::new(Opts::new($name, $help), $labels)
                .expect("metric definition must be valid");
            global_registry()
                .register(Box::new(v.clone()))
                .expect("metric registration must succeed");
            v
        })
    }};
}

macro_rules! lazy_histogram_vec {
    ($name:expr, $help:expr, $labels:expr $(,)?) => {{
        static STORAGE: OnceLock<HistogramVec> = OnceLock::new();
        STORAGE.get_or_init(|| {
            let v = HistogramVec::new(
                HistogramOpts::new($name, $help).buckets(LATENCY_BUCKETS.to_vec()),
                $labels,
            )
            .expect("metric definition must be valid");
            global_registry()
                .register(Box::new(v.clone()))
                .expect("metric registration must succeed");
            v
        })
    }};
}

macro_rules! lazy_gauge {
    ($name:expr, $help:expr $(,)?) => {{
        static STORAGE: OnceLock<IntGauge> = OnceLock::new();
        STORAGE.get_or_init(|| {
            let g = IntGauge::with_opts(Opts::new($name, $help))
                .expect("gauge definition must be valid");
            global_registry()
                .register(Box::new(g.clone()))
                .expect("gauge registration must succeed");
            g
        })
    }};
}

pub fn inc_messages_published(title: &str) {
    lazy_counter_vec!(
        "rex_messages_published_total",
        "Messages accepted by the publish path",
        &["title"],
    )
    .with_label_values(&[title])
    .inc();
}

pub fn inc_messages_delivered(title: &str, target: &str) {
    lazy_counter_vec!(
        "rex_messages_delivered_total",
        "Messages delivered to subscribers",
        &["title", "target"],
    )
    .with_label_values(&[title, target])
    .inc();
}

pub fn inc_messages_failed(reason: &str) {
    lazy_counter_vec!(
        "rex_messages_failed_total",
        "Messages that failed processing",
        &["reason"],
    )
    .with_label_values(&[reason])
    .inc();
}

pub fn inc_forward_failures(reason: &str) {
    lazy_counter_vec!(
        "rex_forward_failures_total",
        "Cross-node forward attempts that failed",
        &["reason"],
    )
    .with_label_values(&[reason])
    .inc();
}

pub fn inc_bytes_in(transport: &str, n: u64) {
    lazy_counter_vec!(
        "rex_bytes_in_total",
        "Bytes received from clients",
        &["transport"],
    )
    .with_label_values(&[transport])
    .inc_by(n);
}

pub fn inc_bytes_out(transport: &str, n: u64) {
    lazy_counter_vec!(
        "rex_bytes_out_total",
        "Bytes sent to clients",
        &["transport"],
    )
    .with_label_values(&[transport])
    .inc_by(n);
}

pub fn observe_publish_latency(title: &str, secs: f64) {
    lazy_histogram_vec!(
        "rex_publish_latency_seconds",
        "End-to-end publish latency in seconds",
        &["title"],
    )
    .with_label_values(&[title])
    .observe(secs);
}

pub fn observe_deliver_latency(title: &str, secs: f64) {
    lazy_histogram_vec!(
        "rex_deliver_latency_seconds",
        "Publish-to-subscriber enqueue latency in seconds",
        &["title"],
    )
    .with_label_values(&[title])
    .observe(secs);
}

/// Count one command processed by the dispatch table, labelled by command
/// id and result. `result` is "ok" or "err" - call sites derive it from
/// `Result::is_ok()`. Cardinality: ~16 commands x 2 results.
pub fn inc_commands_total(command: &str, result: &str) {
    lazy_counter_vec!(
        "rex_commands_total",
        "Commands processed by the dispatch table",
        &["command", "result"],
    )
    .with_label_values(&[command, result])
    .inc();
}

/// Record wall-clock time spent in a command handler, labelled by command.
/// Latency is recorded regardless of `ok`/`err` outcome; splitting the
/// label would double cardinality without informative value.
pub fn observe_command_duration(command: &str, secs: f64) {
    lazy_histogram_vec!(
        "rex_command_duration_seconds",
        "Wall-clock time spent in a command handler",
        &["command"],
    )
    .with_label_values(&[command])
    .observe(secs);
}

pub fn set_clients_connected(n: i64) {
    lazy_gauge!("rex_clients_connected", "Currently connected clients",).set(n);
}

pub fn set_titles_active(n: i64) {
    lazy_gauge!(
        "rex_titles_active",
        "Distinct titles with at least one subscriber",
    )
    .set(n);
}

pub fn set_pending_acks(n: i64) {
    lazy_gauge!(
        "rex_pending_acks",
        "Pending acknowledgements awaiting reply",
    )
    .set(n);
}

pub fn set_cluster_peers(n: i64) {
    lazy_gauge!("rex_cluster_peers", "Known cluster peers (excludes self)",).set(n);
}

pub fn inc_client_state_restore(result: &str) {
    lazy_counter_vec!(
        "rex_client_state_restore_total",
        "ClientStateStore restore operations at startup, labelled by outcome",
        &["result"],
    )
    .with_label_values(&[result])
    .inc();
}

pub fn set_client_state_ghosts_current(n: i64) {
    lazy_gauge!(
        "rex_client_state_ghosts_current",
        "Number of ghost entries currently in the registry",
    )
    .set(n);
}

pub fn observe_client_state_save_latency(secs: f64) {
    lazy_histogram_vec!(
        "rex_client_state_save_latency_seconds",
        "Latency of ClientStateStore::save calls",
        &["op"],
    )
    .with_label_values(&["save"])
    .observe(secs);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_exposes_all_metrics() {
        let reg = global_registry();
        // Touch each metric so it appears in the registry.
        inc_messages_published("t1");
        inc_messages_delivered("t1", "local");
        inc_messages_failed("parse");
        inc_forward_failures("unreachable");
        inc_bytes_in("tcp", 100);
        inc_bytes_out("tcp", 100);
        observe_publish_latency("t1", 0.001);
        observe_deliver_latency("t1", 0.001);
        set_clients_connected(1);
        set_titles_active(1);
        set_pending_acks(0);
        set_cluster_peers(0);

        let metric_families = reg.gather();
        let names: Vec<String> = metric_families
            .iter()
            .map(|m| m.name().to_string())
            .collect();
        for expected in [
            "rex_messages_published_total",
            "rex_messages_delivered_total",
            "rex_messages_failed_total",
            "rex_forward_failures_total",
            "rex_bytes_in_total",
            "rex_bytes_out_total",
            "rex_publish_latency_seconds",
            "rex_deliver_latency_seconds",
            "rex_clients_connected",
            "rex_titles_active",
            "rex_pending_acks",
            "rex_cluster_peers",
        ] {
            assert!(
                names.iter().any(|n| n == expected),
                "metric {} not in registry (have: {:?})",
                expected,
                names
            );
        }
    }

    #[test]
    fn inc_commands_total_increments_for_label() {
        inc_commands_total("Title", "ok");
        inc_commands_total("Title", "ok");
        let count = command_counter_value("Title", "ok");
        assert!(count >= 2.0, "expected counter >= 2, got {}", count);
    }

    #[test]
    fn observe_command_duration_records_value() {
        observe_command_duration("Title", 0.000123);
        let count = command_histogram_count("Title");
        assert!(
            count >= 1,
            "expected histogram sample count >= 1, got {}",
            count
        );
    }

    #[test]
    fn result_label_distinguishes_ok_err() {
        inc_commands_total("Title", "ok");
        inc_commands_total("Title", "err");
        let ok = command_counter_value("Title", "ok");
        let err = command_counter_value("Title", "err");
        assert!(ok >= 1.0, "expected ok counter >= 1, got {}", ok);
        assert!(err >= 1.0, "expected err counter >= 1, got {}", err);
    }

    fn command_counter_value(command: &str, result: &str) -> f64 {
        for mf in global_registry().gather() {
            if mf.name() != "rex_commands_total" {
                continue;
            }
            for metric in mf.get_metric() {
                let labels: Vec<&str> = metric
                    .get_label()
                    .iter()
                    .map(|p| p.name())
                    .filter(|n| *n == "command" || *n == "result")
                    .collect();
                if labels.len() != 2 {
                    continue;
                }
                let mut cmd = None;
                let mut res = None;
                for p in metric.get_label() {
                    match p.name() {
                        "command" => cmd = Some(p.value()),
                        "result" => res = Some(p.value()),
                        _ => {}
                    }
                }
                if cmd == Some(command) && res == Some(result) {
                    return metric.get_counter().value();
                }
            }
        }
        0.0
    }

    fn command_histogram_count(command: &str) -> u64 {
        for mf in global_registry().gather() {
            if mf.name() != "rex_command_duration_seconds" {
                continue;
            }
            for metric in mf.get_metric() {
                for p in metric.get_label() {
                    if p.name() == "command" && p.value() == command {
                        return metric.get_histogram().get_sample_count();
                    }
                }
            }
        }
        0
    }
}
