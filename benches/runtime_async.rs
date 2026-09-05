mod common;

use common::{criterion_config, AsyncStyle};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use futures_lite::future::block_on;
use slot_graph::RunInputs;

macro_rules! bench_async_case {
    ($group:expr, $mode:literal, $shape:expr, $builder:expr, $expected:expr) => {{
        let (graph, output) = $builder;
        let version = graph.compile().unwrap();

        // Exercise the fresh path before timing it, then independently verify
        // the first reusable execution. The latter is intentionally outside
        // Criterion so runner initialization cannot hide a bad result.
        let mut fresh_preflight = block_on(version.execute(RunInputs::new())).unwrap();
        assert_eq!(*fresh_preflight.take_output(output).unwrap(), $expected);

        let mut runner = version.runner();
        let mut reusable_preflight = block_on(runner.execute(RunInputs::new())).unwrap();
        assert_eq!(*reusable_preflight.take_output(output).unwrap(), $expected);

        $group.bench_function(format!("{}/fresh/{}/100", $mode, $shape), |b| {
            b.iter(|| {
                let mut report = block_on(version.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
        $group.bench_function(format!("{}/reusable/{}/100", $mode, $shape), |b| {
            b.iter(|| {
                let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
    }};
}

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_async");
    for (name, style) in [
        ("ready", AsyncStyle::Ready),
        ("pending_once", AsyncStyle::PendingOnce),
        ("mixed_80_sync_20_async", AsyncStyle::Mixed),
    ] {
        bench_async_case!(
            group,
            "local",
            name,
            common::build_local_async_chain(100, style),
            100
        );
    }
    bench_async_case!(
        group,
        "local",
        "wake_storm_independent",
        common::build_local_async_independent(100, AsyncStyle::WakeStorm),
        99
    );

    for (name, style) in [
        ("ready", AsyncStyle::Ready),
        ("pending_once", AsyncStyle::PendingOnce),
        ("mixed_80_sync_20_async", AsyncStyle::Mixed),
    ] {
        bench_async_case!(
            group,
            "send",
            name,
            common::build_send_async_chain(100, style),
            100
        );
    }
    bench_async_case!(
        group,
        "send",
        "wake_storm_independent",
        common::build_send_async_independent(100, AsyncStyle::WakeStorm),
        99
    );
    group.finish();
}

criterion_group! { name = benches; config = criterion_config(); targets = benchmarks }
criterion_main!(benches);
