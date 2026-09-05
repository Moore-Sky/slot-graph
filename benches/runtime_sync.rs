mod common;

use common::{criterion_config, IoStyle};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use futures_lite::future::block_on;
use slot_graph::{Graph, Local, NodeOutputs, OutputSlot, RunInputs};

fn build_local_named_renderer_frame(
    width: usize,
    layers: usize,
) -> (Graph<Local>, OutputSlot<u64>) {
    assert!(width > 0 && layers > 0);
    let mut graph = Graph::<Local>::new();
    let mut previous = Vec::with_capacity(width);
    for index in 0..width {
        previous.push(
            graph
                .add_sync(
                    format!("layer_0_node_{index}"),
                    common::source_schema(),
                    move |_, _| {
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", index as u64 + 1);
                        Ok(outputs)
                    },
                )
                .unwrap(),
        );
    }

    for layer in 1..layers {
        let mut current = Vec::with_capacity(width);
        for index in 0..width {
            let node = graph
                .add_sync(
                    format!("layer_{layer}_node_{index}"),
                    common::many_schema(),
                    move |_, inputs| {
                        let value = inputs
                            .many::<u64>("values")?
                            .iter()
                            .fold(index as u64, |sum, value| sum.wrapping_add(**value));
                        let mut outputs = NodeOutputs::new();
                        outputs.insert("value", value);
                        Ok(outputs)
                    },
                )
                .unwrap();
            let target = graph.input::<u64>(node, "values").unwrap();
            graph
                .collect_into(previous.iter().copied(), target)
                .unwrap();
            current.push(node);
        }
        previous = current;
    }

    for &node in &previous {
        graph.set_active(node, true).unwrap();
    }
    let output = graph
        .output::<u64>(*previous.last().unwrap(), "value")
        .unwrap();
    (graph, output)
}

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("runtime_sync");
    for io in [IoStyle::Named, IoStyle::Bound] {
        let label = match io {
            IoStyle::Named => "named",
            IoStyle::Bound => "bound",
        };
        let (graph, output) = common::build_local_chain(100, 100, io);
        let version = graph.compile().unwrap();
        let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
        assert_eq!(*preflight.take_output(output).unwrap(), 100);
        group.bench_function(format!("local/fresh/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(version.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
        let mut runner = version.runner();
        group.bench_function(format!("local/reusable/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
    }
    for io in [IoStyle::Named, IoStyle::Bound] {
        let label = match io {
            IoStyle::Named => "named",
            IoStyle::Bound => "bound",
        };
        let (graph, output) = common::build_send_chain(100, io);
        let version = graph.compile().unwrap();
        let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
        assert_eq!(*preflight.take_output(output).unwrap(), 100);
        group.bench_function(format!("send/fresh/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(version.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
        let mut runner = version.runner();
        group.bench_function(format!("send/reusable/{label}/100"), |b| {
            b.iter(|| {
                let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
                black_box(*report.take_output(output).unwrap());
            });
        });
    }
    let (graph, output) = common::build_local_renderer_frame(10, 10);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(
        *preflight.take_output(output).unwrap(),
        common::renderer_expected(10, 10)
    );
    let mut runner = version.runner();
    group.bench_function("local/reusable/renderer_frame/bound/100", |b| {
        b.iter(|| {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });
    let (graph, output) = build_local_named_renderer_frame(10, 10);
    let version = graph.compile().unwrap();
    let mut preflight = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(
        *preflight.take_output(output).unwrap(),
        common::renderer_expected(10, 10)
    );
    let mut runner = version.runner();
    group.bench_function("local/reusable/renderer_frame/named/100", |b| {
        b.iter(|| {
            let mut report = block_on(runner.execute(RunInputs::new())).unwrap();
            black_box(*report.take_output(output).unwrap());
        });
    });
    group.finish();
}

criterion_group! { name = benches; config = criterion_config(); targets = benchmarks }
criterion_main!(benches);
