mod common;

use common::{criterion_config, IoStyle};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("compile");
    for &size in &[100_usize, 1_000, 10_000] {
        let (full, _) = common::build_local_chain(size, size, IoStyle::Bound);
        group.bench_function(format!("full/{size}"), |b| {
            b.iter(|| black_box(full.compile().unwrap()));
        });
        let active = (size / 10).max(1);
        let (partial, _) = common::build_local_chain(size, active, IoStyle::Bound);
        group.bench_function(format!("active_10_percent/{size}"), |b| {
            b.iter(|| black_box(partial.compile().unwrap()));
        });
    }
    group.finish();
}

criterion_group! { name = benches; config = criterion_config(); targets = benchmarks }
criterion_main!(benches);
