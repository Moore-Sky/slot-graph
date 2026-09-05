//! Public API contract tests for declaration-graph editing and compilation.
//!
//! These tests intentionally describe the 0.3 contract before the execution
//! engine exists.  They are ignored rather than mocked: when enabled, every
//! assertion exercises only the crate's public API and must pass unchanged.

use slot_graph::{
    Cardinality, CompileErrorKind, EditErrorKind, ExecutionGraphVersion, Graph, InputSpec, Local,
    LocalTaskResult, NodeId, NodeInputs, NodeOutputs, OutputSpec, Presence, RunInputs, Schema,
    SlotId, SlotTypeId, Task, TaskContext,
};
use std::any::Any;

fn empty_task(_: TaskContext<Local>, _: NodeInputs<Local>) -> LocalTaskResult {
    Ok(NodeOutputs::empty())
}

fn source(graph: &mut Graph<Local>, name: &str, output: &str) -> NodeId {
    graph
        .add_sync(
            name,
            Schema::new(vec![], vec![OutputSpec::new::<u32>(output)]),
            empty_task,
        )
        .expect("test graph schema is valid")
}

fn sink_one(graph: &mut Graph<Local>, name: &str, input: &str) -> NodeId {
    graph
        .add_sync(
            name,
            Schema::new(vec![InputSpec::required_one::<u32>(input)], vec![]),
            empty_task,
        )
        .expect("test graph schema is valid")
}

fn sink_many(graph: &mut Graph<Local>, name: &str, input: &str) -> NodeId {
    graph
        .add_sync(
            name,
            Schema::new(vec![InputSpec::required_many::<u32>(input)], vec![]),
            empty_task,
        )
        .expect("test graph schema is valid")
}

fn input_with_id<T: Any>(
    id: u64,
    name: &str,
    presence: Presence,
    cardinality: Cardinality,
) -> InputSpec {
    InputSpec {
        id: SlotId::new(id),
        name: name.to_owned(),
        value_type: SlotTypeId::of::<T>(),
        presence,
        cardinality,
        auto_collect: false,
    }
}

fn assert_compile_kind(graph: &Graph<Local>, expected: CompileErrorKind) {
    match graph.compile() {
        Err(error) => assert_eq!(error.kind, expected),
        Ok(_) => panic!("compile unexpectedly succeeded"),
    }
}

fn assert_edit_kind<T>(result: Result<T, slot_graph::EditError>, expected: EditErrorKind) {
    match result {
        Err(error) => assert_eq!(error.kind, expected),
        Ok(_) => panic!("edit unexpectedly succeeded"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn build_connect_activate_and_compile_a_simple_graph() {
    let mut graph = Graph::<Local>::new();
    let producer = source(&mut graph, "producer", "value");
    let consumer = sink_one(&mut graph, "consumer", "value");

    graph
        .connect(producer.output("value"), consumer.input("value"))
        .unwrap();
    graph.set_active(consumer, true).unwrap();

    let version: ExecutionGraphVersion<Local> = graph.compile().unwrap();
    assert_eq!(version.graph_id(), graph.graph_id());
}

#[test]
#[ignore = "implementation pending"]
fn compile_rejects_a_graph_without_an_active_target() {
    let mut graph = Graph::<Local>::new();
    source(&mut graph, "source", "value");

    assert_compile_kind(&graph, CompileErrorKind::NoActiveTarget);
}

#[test]
#[ignore = "implementation pending"]
fn selected_closure_ignores_an_inactive_missing_required_branch() {
    let mut graph = Graph::<Local>::new();
    let source_node = source(&mut graph, "source", "value");
    let valid = sink_one(&mut graph, "valid", "value");
    let incomplete = sink_one(&mut graph, "incomplete", "value");
    graph
        .connect(source_node.output("value"), valid.input("value"))
        .unwrap();
    graph.set_active(valid, true).unwrap();

    graph
        .compile()
        .expect("inactive incomplete branch must not block compile");

    graph.set_active(incomplete, true).unwrap();
    assert_compile_kind(&graph, CompileErrorKind::MissingRequiredInput);
}

#[test]
#[ignore = "implementation pending"]
fn selected_closure_reports_a_cycle_only_after_that_branch_becomes_active() {
    let mut graph = Graph::<Local>::new();
    let root = source(&mut graph, "root", "root");
    let selected = sink_one(&mut graph, "selected", "root");
    let a = graph
        .add_sync(
            "cycle_a",
            Schema::new(
                vec![InputSpec::required_one::<u32>("from_b")],
                vec![OutputSpec::new::<u32>("to_b")],
            ),
            empty_task,
        )
        .unwrap();
    let b = graph
        .add_sync(
            "cycle_b",
            Schema::new(
                vec![InputSpec::required_one::<u32>("from_a")],
                vec![OutputSpec::new::<u32>("to_a")],
            ),
            empty_task,
        )
        .unwrap();
    graph
        .connect(root.output("root"), selected.input("root"))
        .unwrap();
    graph.connect(a.output("to_b"), b.input("from_a")).unwrap();
    graph.connect(b.output("to_a"), a.input("from_b")).unwrap();
    graph.set_active(selected, true).unwrap();

    graph
        .compile()
        .expect("inactive cycle must not block the selected closure");
    graph.set_active(a, true).unwrap();
    assert_compile_kind(&graph, CompileErrorKind::CycleDetected);
}

#[test]
#[ignore = "implementation pending"]
fn connect_rejects_handles_from_another_graph() {
    let mut left = Graph::<Local>::new();
    let mut right = Graph::<Local>::new();
    let producer = source(&mut left, "producer", "value");
    let consumer = sink_one(&mut right, "consumer", "value");

    assert_edit_kind(
        left.connect(producer.output("value"), consumer.input("value")),
        EditErrorKind::ForeignHandle,
    );
}

#[test]
#[ignore = "implementation pending"]
fn removed_node_and_its_incident_edge_become_stale() {
    let mut graph = Graph::<Local>::new();
    let producer = source(&mut graph, "producer", "value");
    let consumer = sink_one(&mut graph, "consumer", "value");
    let edge = graph
        .connect(producer.output("value"), consumer.input("value"))
        .unwrap();

    let report = graph.remove_node(producer).unwrap();
    assert_eq!(report.removed_edges, vec![edge]);
    assert_edit_kind(graph.disconnect(edge), EditErrorKind::StaleEdgeId);
    assert_edit_kind(graph.set_active(producer, true), EditErrorKind::StaleNodeId);
}

#[test]
#[ignore = "implementation pending"]
fn connect_rejects_duplicate_edges_and_one_input_overflow() {
    let mut graph = Graph::<Local>::new();
    let first = source(&mut graph, "first", "value");
    let second = source(&mut graph, "second", "value");
    let consumer = sink_one(&mut graph, "consumer", "value");
    graph
        .connect(first.output("value"), consumer.input("value"))
        .unwrap();

    assert_edit_kind(
        graph.connect(first.output("value"), consumer.input("value")),
        EditErrorKind::DuplicateEdge,
    );
    assert_edit_kind(
        graph.connect(second.output("value"), consumer.input("value")),
        EditErrorKind::CardinalityOverflow,
    );
}

#[test]
#[ignore = "implementation pending"]
fn reconnect_is_atomic_when_new_source_would_duplicate_an_existing_many_edge() {
    let mut graph = Graph::<Local>::new();
    let first = source(&mut graph, "first", "value");
    let second = source(&mut graph, "second", "value");
    let consumer = sink_many(&mut graph, "consumer", "values");
    let first_edge = graph
        .connect(first.output("value"), consumer.input("values"))
        .unwrap();
    let second_edge = graph
        .connect(second.output("value"), consumer.input("values"))
        .unwrap();

    assert_edit_kind(
        graph.reconnect(second_edge, first.output("value")),
        EditErrorKind::DuplicateEdge,
    );
    graph
        .disconnect(second_edge)
        .expect("failed reconnect leaves the original edge intact");
    graph.disconnect(first_edge).unwrap();
}

#[test]
#[ignore = "implementation pending"]
fn reconnect_returns_an_edge_that_can_be_used_for_the_replaced_connection() {
    let mut graph = Graph::<Local>::new();
    let first = source(&mut graph, "first", "value");
    let second = source(&mut graph, "second", "value");
    let consumer = sink_one(&mut graph, "consumer", "value");
    let edge = graph
        .connect(first.output("value"), consumer.input("value"))
        .unwrap();

    let replacement = graph.reconnect(edge, second.output("value")).unwrap();
    graph.disconnect(replacement).unwrap();
}

#[test]
#[ignore = "implementation pending"]
fn auto_connect_prefers_exact_name_over_the_type_only_fallback() {
    let mut graph = Graph::<Local>::new();
    let source_node = graph
        .add_sync(
            "source",
            Schema::new(
                vec![],
                vec![
                    OutputSpec::new::<u32>("wanted"),
                    OutputSpec::new::<u32>("other"),
                ],
            ),
            empty_task,
        )
        .unwrap();
    let target = sink_one(&mut graph, "target", "wanted");

    let report = graph.connect_nodes(source_node, target).unwrap();
    assert_eq!(report.edges.len(), 1);
    assert!(report.unmatched_required.is_empty());
}

#[test]
#[ignore = "implementation pending"]
fn auto_connect_uses_type_only_fallback_only_when_it_is_unique() {
    let mut graph = Graph::<Local>::new();
    let source_node = source(&mut graph, "source", "different_name");
    let target = sink_one(&mut graph, "target", "wanted");

    let report = graph.connect_nodes(source_node, target).unwrap();
    assert_eq!(report.edges.len(), 1);
    assert!(report.unmatched_required.is_empty());
}

#[test]
#[ignore = "implementation pending"]
fn auto_connect_does_not_use_slot_id_as_a_cross_node_matching_key() {
    let mut graph = Graph::<Local>::new();
    let source_node = graph
        .add_sync(
            "source",
            Schema::new(
                vec![],
                vec![
                    OutputSpec {
                        id: SlotId::new(7),
                        name: "left".into(),
                        value_type: SlotTypeId::of::<u32>(),
                    },
                    OutputSpec {
                        id: SlotId::new(8),
                        name: "right".into(),
                        value_type: SlotTypeId::of::<u32>(),
                    },
                ],
            ),
            empty_task,
        )
        .unwrap();
    let target = graph
        .add_sync(
            "target",
            Schema::new(
                vec![input_with_id::<u32>(
                    7,
                    "target_name",
                    Presence::Required,
                    Cardinality::One,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();

    assert_edit_kind(
        graph.connect_nodes(source_node, target),
        EditErrorKind::AmbiguousAutoMatch,
    );
}

#[test]
#[ignore = "implementation pending"]
fn auto_connect_is_atomic_when_one_input_is_ambiguous() {
    let mut graph = Graph::<Local>::new();
    let source_node = graph
        .add_sync(
            "source",
            Schema::new(
                vec![],
                vec![
                    OutputSpec::new::<u32>("ambiguous_a"),
                    OutputSpec::new::<u32>("ambiguous_b"),
                    OutputSpec::new::<String>("text"),
                ],
            ),
            empty_task,
        )
        .unwrap();
    let target = graph
        .add_sync(
            "target",
            Schema::new(
                vec![
                    InputSpec::required_one::<u32>("number"),
                    InputSpec::required_one::<String>("text"),
                ],
                vec![],
            ),
            empty_task,
        )
        .unwrap();

    assert_edit_kind(
        graph.connect_nodes(source_node, target),
        EditErrorKind::AmbiguousAutoMatch,
    );
    graph
        .connect(source_node.output("text"), target.input("text"))
        .expect("failed plan must not have added text edge");
}

#[test]
#[ignore = "implementation pending"]
fn auto_collect_many_skips_existing_edges_and_reports_remaining_connections() {
    let mut graph = Graph::<Local>::new();
    let source_node = graph
        .add_sync(
            "source",
            Schema::new(
                vec![],
                vec![
                    OutputSpec::new::<u32>("a"),
                    OutputSpec::new::<u32>("b"),
                    OutputSpec::new::<u32>("c"),
                    OutputSpec::new::<String>("other"),
                ],
            ),
            empty_task,
        )
        .unwrap();
    let target = graph
        .add_sync(
            "target",
            Schema::new(
                vec![InputSpec::required_many::<u32>("items").auto_collect(true)],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    graph
        .connect(source_node.output("b"), target.input("items"))
        .unwrap();

    let report = graph.connect_nodes(source_node, target).unwrap();
    assert_eq!(
        report.edges.len(),
        2,
        "a and c are added; the existing b edge is skipped"
    );
    assert!(report.unmatched_required.is_empty());
}

#[test]
#[ignore = "implementation pending"]
fn auto_connect_reports_unmatched_required_input_and_compile_makes_it_an_error() {
    let mut graph = Graph::<Local>::new();
    let source_node = source(&mut graph, "source", "text");
    let target = sink_one(&mut graph, "target", "number");
    let report = graph.connect_nodes(source_node, target).unwrap();
    assert_eq!(report.unmatched_required.len(), 1);

    graph.set_active(target, true).unwrap();
    assert_compile_kind(&graph, CompileErrorKind::MissingRequiredInput);
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_preserves_compatible_edge_but_makes_old_slot_handles_stale() {
    let mut graph = Graph::<Local>::new();
    let producer = source(&mut graph, "producer", "value");
    let consumer = graph
        .add_sync(
            "consumer",
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "old_name",
                    Presence::Required,
                    Cardinality::One,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let old_input = graph.input::<u32>(consumer, "old_name").unwrap();
    let edge = graph
        .connect(producer.output("value"), consumer.input("old_name"))
        .unwrap();

    let report = graph
        .replace_schema(
            consumer,
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "new_name",
                    Presence::Required,
                    Cardinality::One,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert!(report.removed_edges.is_empty());
    assert_edit_kind(
        graph.connect(producer.output("value"), old_input),
        EditErrorKind::StaleSlotHandle,
    );
    graph
        .disconnect(edge)
        .expect("compatible edge keeps its EdgeId");
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_removes_incompatible_edge_and_makes_its_id_stale() {
    let mut graph = Graph::<Local>::new();
    let producer = source(&mut graph, "producer", "value");
    let consumer = graph
        .add_sync(
            "consumer",
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "input",
                    Presence::Required,
                    Cardinality::One,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let edge = graph
        .connect(producer.output("value"), consumer.input("input"))
        .unwrap();

    let report = graph
        .replace_schema(
            consumer,
            Schema::new(
                vec![input_with_id::<u32>(
                    99,
                    "input",
                    Presence::Required,
                    Cardinality::One,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_eq!(report.removed_edges, vec![edge]);
    assert_edit_kind(graph.disconnect(edge), EditErrorKind::StaleEdgeId);
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_many_to_one_conflict_fails_without_partially_mutating_the_graph() {
    let mut graph = Graph::<Local>::new();
    let first = source(&mut graph, "first", "value");
    let second = source(&mut graph, "second", "value");
    let consumer = graph
        .add_sync(
            "consumer",
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "values",
                    Presence::Required,
                    Cardinality::Many,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let first_edge = graph
        .connect(first.output("value"), consumer.input("values"))
        .unwrap();
    let second_edge = graph
        .connect(second.output("value"), consumer.input("values"))
        .unwrap();

    assert_edit_kind(
        graph.replace_schema(
            consumer,
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "values",
                    Presence::Required,
                    Cardinality::One,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        ),
        EditErrorKind::CardinalityOverflow,
    );
    graph
        .disconnect(first_edge)
        .expect("failed replacement preserves first edge");
    graph
        .disconnect(second_edge)
        .expect("failed replacement preserves second edge");
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_preserves_an_exposed_input_only_when_its_full_contract_is_unchanged() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "consumer",
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let input = graph.input::<u32>(node, "input").unwrap();
    let exposed = graph.expose_input(input).unwrap();
    graph.set_active(node, true).unwrap();

    graph
        .replace_schema(
            node,
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "renamed",
                    Presence::Optional,
                    Cardinality::One,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    let version = graph.compile().unwrap();
    let mut inputs = RunInputs::<Local>::new();
    inputs.insert(exposed, 7_u32).unwrap();
    version
        .start(inputs)
        .expect("unchanged exposed-input contract is preserved");
}

#[test]
#[ignore = "implementation pending"]
fn replace_schema_reports_an_exposed_input_when_cardinality_changes() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "consumer",
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "input",
                    Presence::Optional,
                    Cardinality::One,
                )],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let input = graph.input::<u32>(node, "input").unwrap();
    let exposed = graph.expose_input(input).unwrap();
    graph.set_active(node, true).unwrap();

    let report = graph
        .replace_schema(
            node,
            Schema::new(
                vec![input_with_id::<u32>(
                    41,
                    "input",
                    Presence::Optional,
                    Cardinality::Many,
                )],
                vec![],
            ),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_eq!(report.removed_inputs.len(), 1);
    let version = graph.compile().unwrap();
    let mut inputs = RunInputs::<Local>::new();
    inputs.insert(exposed, 7_u32).unwrap();
    match version.start(inputs) {
        Err(error) => assert_eq!(error.kind, slot_graph::StartErrorKind::UnexpectedRunInput),
        Ok(_) => panic!("new version must reject a removed exposed-input key"),
    }
}
