mod common;

use common::{add_local_producer, criterion_config, many_schema, step_schema};
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use futures_lite::future::block_on;
use slot_graph::{Graph, InputSlot, Local, NodeId, NodeOutputs, OutputSlot, RunInputs, Task};

fn add_step(graph: &mut Graph<Local>, name: impl Into<String>) -> NodeId {
    graph
        .add_sync(name, step_schema(), |_, inputs| {
            let value = inputs.required::<u64>("value")?;
            let mut outputs = NodeOutputs::new();
            outputs.insert("value", *value + 1);
            Ok(outputs)
        })
        .unwrap()
}

fn fan_out_fan_in() -> (Graph<Local>, OutputSlot<u64>) {
    let mut graph = Graph::<Local>::new();
    let source = add_local_producer(&mut graph, "source".into(), 1);
    let mut middle = Vec::with_capacity(98);
    for index in 0..98 {
        let node = add_step(&mut graph, format!("middle_{index}"));
        graph.connect_nodes(source, node).unwrap();
        middle.push(node);
    }
    let join = graph
        .add_sync("join", many_schema(), |_, inputs| {
            let sum: u64 = inputs
                .many::<u64>("values")?
                .iter()
                .map(|value| **value)
                .sum();
            let mut outputs = NodeOutputs::new();
            outputs.insert("value", sum);
            Ok(outputs)
        })
        .unwrap();
    let input = graph.input::<u64>(join, "values").unwrap();
    graph.collect_into(middle, input).unwrap();
    graph.set_active(join, true).unwrap();
    let output = graph.output::<u64>(join, "value").unwrap();
    (graph, output)
}

fn connect_setup() -> (Graph<Local>, Vec<(NodeId, NodeId)>) {
    let mut graph = Graph::<Local>::new();
    let mut pairs = Vec::with_capacity(100);
    for index in 0..100 {
        let source = add_local_producer(&mut graph, format!("source_{index}"), index as u64);
        let target = add_step(&mut graph, format!("target_{index}"));
        pairs.push((source, target));
    }
    (graph, pairs)
}

fn connect_all(graph: &mut Graph<Local>, pairs: &[(NodeId, NodeId)]) -> usize {
    let mut edges = 0;
    for &(source, target) in pairs {
        let report = graph.connect_nodes(source, target).unwrap();
        edges += report.edges.len();
    }
    edges
}

fn assert_connect_nodes_semantics() {
    let (mut graph, pairs) = connect_setup();
    assert_eq!(connect_all(&mut graph, &pairs), 100);
    let outputs: Vec<_> = pairs
        .iter()
        .enumerate()
        .map(|(index, &(_, target))| {
            graph.set_active(target, true).unwrap();
            (
                graph.output::<u64>(target, "value").unwrap(),
                index as u64 + 1,
            )
        })
        .collect();
    let mut report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    for (output, expected) in outputs {
        assert_eq!(*report.take_output(output).unwrap(), expected);
    }
}

fn collect_setup() -> (Graph<Local>, Vec<NodeId>, NodeId, InputSlot<u64>) {
    let mut graph = Graph::<Local>::new();
    let nodes: Vec<_> = (0..100)
        .map(|index| add_local_producer(&mut graph, format!("source_{index}"), index))
        .collect();
    let target = graph
        .add_sync("target", many_schema(), |_, inputs| {
            let sum: u64 = inputs
                .many::<u64>("values")?
                .iter()
                .map(|value| **value)
                .sum();
            let mut outputs = NodeOutputs::new();
            outputs.insert("value", sum);
            Ok(outputs)
        })
        .unwrap();
    let input = graph.input::<u64>(target, "values").unwrap();
    (graph, nodes, target, input)
}

fn assert_collect_into_semantics() {
    let (mut graph, nodes, target, input) = collect_setup();
    assert_eq!(graph.collect_into(nodes, input).unwrap().len(), 100);
    graph.set_active(target, true).unwrap();
    let output = graph.output::<u64>(target, "value").unwrap();
    let mut report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(*report.take_output(output).unwrap(), 4_950);
}

fn replacement_setup() -> (Graph<Local>, NodeId, OutputSlot<u64>) {
    let mut graph = Graph::<Local>::new();
    let source = add_local_producer(&mut graph, "source".into(), 1);
    let middle = add_step(&mut graph, "middle");
    let sink = add_step(&mut graph, "sink");
    graph.connect_nodes(source, middle).unwrap();
    graph.connect_nodes(middle, sink).unwrap();
    for index in 0..97 {
        add_local_producer(&mut graph, format!("unrelated_{index}"), index);
    }
    graph.set_active(sink, true).unwrap();
    let output = graph.output::<u64>(sink, "value").unwrap();
    (graph, middle, output)
}

fn replacement_task() -> Task<Local> {
    Task::<Local>::sync(|_, inputs| {
        let value = inputs.required::<u64>("value")?;
        let mut outputs = NodeOutputs::new();
        outputs.insert("value", *value + 1);
        Ok(outputs)
    })
}

fn assert_replace_schema_semantics() {
    let (mut graph, middle, output) = replacement_setup();
    let replacement = graph
        .replace_schema(middle, step_schema(), replacement_task())
        .unwrap();
    assert!(replacement.removed_edges.is_empty());
    assert!(replacement.removed_inputs.is_empty());
    let mut report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(*report.take_output(output).unwrap(), 3);
}

fn benchmarks(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph_build");
    for &size in &[100_usize, 1_000, 10_000] {
        group.bench_function(format!("chain/{size}"), |b| {
            b.iter(|| black_box(common::build_local_chain(size, size, common::IoStyle::Bound).0));
        });
    }

    let (fan_graph, fan_output) = fan_out_fan_in();
    let mut report = block_on(fan_graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(*report.take_output(fan_output).unwrap(), 196);
    group.bench_function("fan_out_fan_in/100", |b| {
        b.iter(|| black_box(fan_out_fan_in().0));
    });

    assert_connect_nodes_semantics();
    group.bench_function("connect_nodes/100_pairs", |b| {
        b.iter_batched_ref(
            connect_setup,
            |state| {
                let (graph, pairs) = state;
                black_box(connect_all(graph, pairs));
            },
            BatchSize::SmallInput,
        );
    });

    assert_collect_into_semantics();
    group.bench_function("collect_into/100", |b| {
        b.iter_batched_ref(
            collect_setup,
            |state| {
                let (graph, nodes, _, input) = state;
                black_box(graph.collect_into(nodes.iter().copied(), *input).unwrap());
            },
            BatchSize::SmallInput,
        );
    });

    assert_replace_schema_semantics();
    group.bench_function("replace_schema/100_nodes_2_incident_edges", |b| {
        b.iter_batched_ref(
            replacement_setup,
            |state| {
                let (graph, middle, _) = state;
                black_box(
                    graph
                        .replace_schema(*middle, step_schema(), replacement_task())
                        .unwrap(),
                );
            },
            BatchSize::SmallInput,
        );
    });
    group.finish();
}

criterion_group! { name = benches; config = criterion_config(); targets = benchmarks }
criterion_main!(benches);
