//! Immutable-version and schema-replacement public API contracts.
//!
//! The engine is intentionally not implemented yet, so every scenario is
//! ignored. The tests still compile against public APIs and become executable
//! integration tests without rewriting their assertions.

use std::any::Any;

use futures_lite::future::block_on;
use slot_graph::{
    outputs, Cardinality, CompileErrorKind, Graph, InputSpec, Local, LocalTaskResult, NodeInputs,
    NodeOutputs, OutputSlot, OutputSpec, Presence, RunInputs, Schema, SlotId, SlotTypeId, Task,
    TaskContext,
};

fn empty_task(_: TaskContext<Local>, _: NodeInputs<Local>) -> LocalTaskResult {
    Ok(NodeOutputs::empty())
}

fn stable_input<T: Any>(
    id: u64,
    name: &str,
    presence: Presence,
    cardinality: Cardinality,
    auto_collect: bool,
) -> InputSpec {
    InputSpec {
        id: SlotId::new(id),
        name: name.to_owned(),
        value_type: SlotTypeId::of::<T>(),
        presence,
        cardinality,
        auto_collect,
    }
}

fn stable_output<T: Any>(id: u64, name: &str) -> OutputSpec {
    OutputSpec {
        id: SlotId::new(id),
        name: name.to_owned(),
        value_type: SlotTypeId::of::<T>(),
    }
}

fn run_u32(version: &slot_graph::ExecutionGraphVersion<Local>, output: OutputSlot<u32>) -> u32 {
    let report = block_on(version.execute(RunInputs::new())).expect("contract run succeeds");
    **report.output(output).expect("active output is retained")
}

fn assert_compile_kind(graph: &Graph<Local>, expected: CompileErrorKind) {
    match graph.compile() {
        Err(error) => assert_eq!(error.kind, expected),
        Ok(_) => panic!("compile unexpectedly succeeded"),
    }
}

fn assert_start_unexpected(
    version: &slot_graph::ExecutionGraphVersion<Local>,
    inputs: RunInputs<Local>,
) {
    match version.start(inputs) {
        Err(error) => assert_eq!(error.kind, slot_graph::StartErrorKind::UnexpectedRunInput),
        Ok(_) => panic!("removed external-input key was unexpectedly accepted"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn reconnect_creates_a_new_version_without_changing_v1_outputs() {
    let mut graph = Graph::<Local>::new();
    let first = graph
        .add_sync(
            "first",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 1_u32 }),
        )
        .unwrap();
    let second = graph
        .add_sync(
            "second",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 2_u32 }),
        )
        .unwrap();
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(
                vec![InputSpec::required_one::<u32>("value")],
                vec![OutputSpec::new::<u32>("result")],
            ),
            |_task, inputs| Ok(outputs! { "result" => *inputs.required::<u32>("value")? }),
        )
        .unwrap();
    let edge = graph
        .connect(first.output("value"), sink.input("value"))
        .unwrap();
    graph.set_active(sink, true).unwrap();
    let old_output = graph.output::<u32>(sink, "result").unwrap();
    let v1 = graph.compile().unwrap();

    let replacement = graph.reconnect(edge, second.output("value")).unwrap();
    let v2 = graph.compile().unwrap();

    assert_eq!(run_u32(&v1, old_output), 1);
    assert_eq!(run_u32(&v2, old_output), 2);
    graph.disconnect(replacement).unwrap();
}

#[test]
#[ignore = "implementation pending"]
fn schema_rename_and_reorder_preserve_edge_ids_and_many_connection_order() {
    let mut graph = Graph::<Local>::new();
    let first = graph
        .add_sync(
            "first",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 1_u32 }),
        )
        .unwrap();
    let second = graph
        .add_sync(
            "second",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 2_u32 }),
        )
        .unwrap();
    let merge = graph
        .add_sync(
            "merge",
            Schema::new(
                vec![stable_input::<u32>(
                    10,
                    "items",
                    Presence::Required,
                    Cardinality::Many,
                    false,
                )],
                vec![stable_output::<Vec<u32>>(11, "result")],
            ),
            |_task, inputs| {
                Ok(outputs! {
                    "result" => inputs.many::<u32>("items")?
                        .into_iter()
                        .map(|value| *value)
                        .collect::<Vec<_>>()
                })
            },
        )
        .unwrap();
    let first_edge = graph
        .connect(first.output("value"), merge.input("items"))
        .unwrap();
    let second_edge = graph
        .connect(second.output("value"), merge.input("items"))
        .unwrap();
    graph.set_active(merge, true).unwrap();
    let v1 = graph.compile().unwrap();
    let old_output = graph.output::<Vec<u32>>(merge, "result").unwrap();

    let report = graph
        .replace_schema(
            merge,
            Schema::new(
                vec![stable_input::<u32>(
                    10,
                    "renamed_items",
                    Presence::Required,
                    Cardinality::Many,
                    false,
                )],
                vec![
                    stable_output::<bool>(12, "metadata"),
                    stable_output::<Vec<u32>>(11, "renamed_result"),
                ],
            ),
            Task::<Local>::sync(|_task, inputs| {
                Ok(outputs! {
                    "metadata" => true,
                    "renamed_result" => inputs.many::<u32>("renamed_items")?
                        .into_iter()
                        .map(|value| *value)
                        .collect::<Vec<_>>()
                })
            }),
        )
        .unwrap();
    assert!(report.removed_edges.is_empty());
    let v2 = graph.compile().unwrap();
    let new_output = graph.output::<Vec<u32>>(merge, "renamed_result").unwrap();

    let v1_report = block_on(v1.execute(RunInputs::new())).unwrap();
    assert_eq!(v1_report.output(old_output).unwrap().as_ref(), &[1, 2]);
    let v2_report = block_on(v2.execute(RunInputs::new())).unwrap();
    assert_eq!(v2_report.output(new_output).unwrap().as_ref(), &[1, 2]);
    graph.disconnect(first_edge).unwrap();
    graph.disconnect(second_edge).unwrap();
}

#[test]
#[ignore = "implementation pending"]
fn cardinality_replacements_keep_zero_or_one_edge_and_preserve_one_to_many() {
    let mut zero_graph = Graph::<Local>::new();
    let zero = zero_graph
        .add_sync(
            "zero",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "value",
                    Presence::Optional,
                    Cardinality::Many,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    zero_graph
        .replace_schema(
            zero,
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "value",
                    Presence::Optional,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .expect("Many to One with zero edges is valid");

    let mut one_graph = Graph::<Local>::new();
    let source = one_graph
        .add_sync(
            "source",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 1_u32 }),
        )
        .unwrap();
    let sink = one_graph
        .add_sync(
            "sink",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "value",
                    Presence::Required,
                    Cardinality::Many,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let edge = one_graph
        .connect(source.output("value"), sink.input("value"))
        .unwrap();
    one_graph
        .replace_schema(
            sink,
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "value",
                    Presence::Required,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .expect("Many to One with one compatible edge is valid");
    one_graph
        .disconnect(edge)
        .expect("the single compatible edge is preserved");

    let mut widening_graph = Graph::<Local>::new();
    let source = widening_graph
        .add_sync(
            "source",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 1_u32 }),
        )
        .unwrap();
    let sink = widening_graph
        .add_sync(
            "sink",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "value",
                    Presence::Required,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let edge = widening_graph
        .connect(source.output("value"), sink.input("value"))
        .unwrap();
    widening_graph
        .replace_schema(
            sink,
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "value",
                    Presence::Required,
                    Cardinality::Many,
                    false,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .expect("One to Many preserves its compatible edge");
    widening_graph.disconnect(edge).unwrap();
}

#[test]
#[ignore = "implementation pending"]
fn presence_and_auto_collect_changes_only_affect_future_compile_and_connect() {
    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_sync(
            "source",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 1_u32 }),
        )
        .unwrap();
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "values",
                    Presence::Optional,
                    Cardinality::Many,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    graph.set_active(sink, true).unwrap();
    graph
        .compile()
        .expect("optional unconnected input is valid");
    let before = graph.connect_nodes(source, sink).unwrap();
    assert!(before.edges.is_empty());

    graph
        .replace_schema(
            sink,
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "values",
                    Presence::Required,
                    Cardinality::Many,
                    true,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_compile_kind(&graph, CompileErrorKind::MissingRequiredInput);
    let after = graph.connect_nodes(source, sink).unwrap();
    assert_eq!(after.edges.len(), 1);
    graph
        .compile()
        .expect("future auto-connect satisfies the new Required input");
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_identity_change_removes_new_version_input_but_keeps_v1_key() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "node",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let key = graph.expose_input::<u32>(node.input("input")).unwrap();
    graph.set_active(node, true).unwrap();
    let v1 = graph.compile().unwrap();

    let report = graph
        .replace_schema(
            node,
            Schema::new(
                vec![stable_input::<u32>(
                    2,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_eq!(report.removed_inputs.len(), 1);
    let v2 = graph.compile().unwrap();

    let mut old_inputs = RunInputs::<Local>::new();
    old_inputs.insert(key, 7_u32).unwrap();
    v1.start(old_inputs)
        .expect("v1 freezes its old external-input key");
    let mut new_inputs = RunInputs::<Local>::new();
    new_inputs.insert(key, 7_u32).unwrap();
    assert_start_unexpected(&v2, new_inputs);
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_type_and_cardinality_changes_remove_new_version_input_keys() {
    let mut type_graph = Graph::<Local>::new();
    let type_node = type_graph
        .add_sync(
            "type_node",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let type_key = type_graph
        .expose_input::<u32>(type_node.input("input"))
        .unwrap();
    type_graph.set_active(type_node, true).unwrap();
    let type_v1 = type_graph.compile().unwrap();
    let type_report = type_graph
        .replace_schema(
            type_node,
            Schema::new(
                vec![stable_input::<String>(
                    1,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_eq!(type_report.removed_inputs.len(), 1);
    let type_v2 = type_graph.compile().unwrap();
    let mut type_old_inputs = RunInputs::<Local>::new();
    type_old_inputs.insert(type_key, 1_u32).unwrap();
    type_v1.start(type_old_inputs).unwrap();
    let mut type_new_inputs = RunInputs::<Local>::new();
    type_new_inputs.insert(type_key, 1_u32).unwrap();
    assert_start_unexpected(&type_v2, type_new_inputs);

    let mut cardinality_graph = Graph::<Local>::new();
    let cardinality_node = cardinality_graph
        .add_sync(
            "cardinality_node",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let cardinality_key = cardinality_graph
        .expose_input::<u32>(cardinality_node.input("input"))
        .unwrap();
    cardinality_graph
        .set_active(cardinality_node, true)
        .unwrap();
    let cardinality_v1 = cardinality_graph.compile().unwrap();
    let cardinality_report = cardinality_graph
        .replace_schema(
            cardinality_node,
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "input",
                    Presence::Optional,
                    Cardinality::Many,
                    false,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_eq!(cardinality_report.removed_inputs.len(), 1);
    let cardinality_v2 = cardinality_graph.compile().unwrap();
    let mut cardinality_old_inputs = RunInputs::<Local>::new();
    cardinality_old_inputs
        .insert(cardinality_key, 1_u32)
        .unwrap();
    cardinality_v1.start(cardinality_old_inputs).unwrap();
    let mut cardinality_new_inputs = RunInputs::<Local>::new();
    cardinality_new_inputs
        .insert(cardinality_key, 1_u32)
        .unwrap();
    assert_start_unexpected(&cardinality_v2, cardinality_new_inputs);
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_presence_change_removes_the_binding_and_keeps_v1_usable() {
    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_sync(
            "source",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 9_u32 }),
        )
        .unwrap();
    let node = graph
        .add_sync(
            "node",
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let key = graph.expose_input::<u32>(node.input("input")).unwrap();
    graph.set_active(node, true).unwrap();
    let v1 = graph.compile().unwrap();

    let report = graph
        .replace_schema(
            node,
            Schema::new(
                vec![stable_input::<u32>(
                    1,
                    "input",
                    Presence::Required,
                    Cardinality::One,
                    false,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_eq!(report.removed_inputs.len(), 1);
    graph
        .connect(
            source.output("value"),
            graph.input::<u32>(node, "input").unwrap(),
        )
        .unwrap();
    let v2 = graph
        .compile()
        .expect("the new required input is now supplied by its normal producer");

    let mut old_inputs = RunInputs::<Local>::new();
    old_inputs.insert(key, 7_u32).unwrap();
    v1.start(old_inputs)
        .expect("v1 remains usable after declaration replacement");
    let mut new_inputs = RunInputs::<Local>::new();
    new_inputs.insert(key, 7_u32).unwrap();
    assert_start_unexpected(&v2, new_inputs);
}

#[test]
#[ignore = "implementation pending"]
fn replace_task_keeps_schema_and_edges_while_old_version_keeps_old_factory() {
    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_sync(
            "source",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 1_u32 }),
        )
        .unwrap();
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(
                vec![InputSpec::required_one::<u32>("value")],
                vec![OutputSpec::new::<u32>("result")],
            ),
            |_task, inputs| Ok(outputs! { "result" => *inputs.required::<u32>("value")? }),
        )
        .unwrap();
    let edge = graph
        .connect(source.output("value"), sink.input("value"))
        .unwrap();
    graph.set_active(sink, true).unwrap();
    let output = graph.output::<u32>(sink, "result").unwrap();
    let v1 = graph.compile().unwrap();

    graph
        .replace_task(
            sink,
            Task::<Local>::sync(|_task, inputs| {
                Ok(outputs! { "result" => *inputs.required::<u32>("value")? + 10 })
            }),
        )
        .unwrap();
    let v2 = graph.compile().unwrap();

    assert_eq!(run_u32(&v1, output), 1);
    assert_eq!(run_u32(&v2, output), 11);
    graph
        .disconnect(edge)
        .expect("replace_task does not remove schema-compatible edges");
}

#[test]
#[ignore = "implementation pending"]
fn failed_compile_never_invalidates_a_previously_compiled_version() {
    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_sync(
            "source",
            Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]),
            |_task, _inputs| Ok(outputs! { "value" => 5_u32 }),
        )
        .unwrap();
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(
                vec![InputSpec::required_one::<u32>("value")],
                vec![OutputSpec::new::<u32>("result")],
            ),
            |_task, inputs| Ok(outputs! { "result" => *inputs.required::<u32>("value")? }),
        )
        .unwrap();
    let edge = graph
        .connect(source.output("value"), sink.input("value"))
        .unwrap();
    graph.set_active(sink, true).unwrap();
    let output = graph.output::<u32>(sink, "result").unwrap();
    let v1 = graph.compile().unwrap();

    graph.disconnect(edge).unwrap();
    assert_compile_kind(&graph, CompileErrorKind::MissingRequiredInput);
    assert_eq!(run_u32(&v1, output), 5);
}
