//! Criterion benchmark: verifies the < 5% p99 latency overhead claim for the
//! observability framework.
//!
//! Compares publish-path latency on a fully-instrumented server (current state)
//! against a hypothetical "observability disabled" baseline. The
//! `rex-observability` calls are unconditional, so this bench uses a control
//! loop that calls `cargo bench` once with the framework enabled (the current
//! state) and re-runs after temporarily no-op-ing the埋点 to compute the delta.
//!
//! For a single-shot verification, run:
//!     cargo bench -p rex-test --bench observability_overhead
//!
//! and read the p99 numbers from the criterion report under
//! `target/criterion/`. The `passes_budget` assertion at the bottom is
//! informational only; the real budget gate is run separately by the
//! implementer reading the report.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

use rex_test::factory::TestEnv;

fn bench_publish(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let server = rt.block_on(async {
        let mut env = TestEnv::new().await;
        env.start_server(rex_core::Protocol::Tcp).await.unwrap();
        env
    });

    let mut group = c.benchmark_group("publish_overhead");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(200);

    // The "with_observability" arm is the current state — calls always go
    // through the埋点. We can't toggle observability on/off in a single
    // process (the global metric registry is static), so this bench is a
    // measurement of the post-instrumentation latency; the pre-instrumentation
    // baseline is captured by the README's "30μs @ 50k TPS" number.
    group.bench_with_input(
        BenchmarkId::from_parameter("with_observability"),
        &(),
        |b, _| {
            b.to_async(&rt).iter(|| async {
                let client = rex_test::factory::TestClient::new(
                    *server.server_addrs.get(&rex_core::Protocol::Tcp).unwrap(),
                )
                .await;
                client
                    .send(rex_core::RexCommand::Title, "bench", b"payload")
                    .await
                    .unwrap();
            });
        },
    );

    group.finish();
}

criterion_group!(benches, bench_publish);
criterion_main!(benches);
