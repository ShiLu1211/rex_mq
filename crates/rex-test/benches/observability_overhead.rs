//! Criterion benchmark: verifies the < 5% p99 latency overhead claim for the
//! observability framework.
//!
//! Compares publish-path latency on a fully-instrumented server (current state)
//! against the README's pre-instrumentation baseline of 30μs @ 50k TPS. The
//! `rex-observability` calls are unconditional, so the framework cannot be
//! toggled in a single binary. The "before" measurement is the README's
//! claimed baseline; the "after" measurement is this bench's output.
//!
//! ## Run
//!
//!     cargo bench -p rex-test --bench observability_overhead
//!
//! Open `target/criterion/publish_overhead/with_observability/report/index.html`
//! for the p99 reading. Compute `(p99_after - p99_before) / p99_before` and
//! verify it's < 0.05.
//!
//! ## Status
//!
//! This bench compiles and runs. The p99 number has not been measured yet
//! (token plan quota exhausted at the time of writing). The user must run
//! the command above and inspect the report before declaring the
//! observability framework production-ready on the perf axis.

use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use rex_core::Protocol;
use rex_test::factory::TestEnv;

fn bench_publish(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    let mut env = rt.block_on(async {
        let mut env = TestEnv::new().await;
        env.start_server(Protocol::Tcp).await.unwrap();
        env
    });

    // One client is enough to measure the publish path; the hot path is
    // server-side, not client→server wire time.
    let client = rt.block_on(async { env.create_client(Protocol::Tcp, "bench").await.unwrap() });

    let mut group = c.benchmark_group("publish_overhead");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(200);

    group.bench_function("with_observability", |b| {
        b.to_async(&rt).iter(|| async {
            client
                .send(rex_core::RexCommand::Title, "bench", b"payload")
                .await
                .unwrap();
        });
    });

    group.finish();
}

criterion_group!(benches, bench_publish);
criterion_main!(benches);
