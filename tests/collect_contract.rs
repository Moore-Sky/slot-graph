//! Contracts for explicit collection into a single `Many` input.
//!
//! These are intentionally ignored until graph editing is implemented.  They
//! use only public APIs so they become executable acceptance tests unchanged.

use futures_lite::future::block_on;
use slot_graph::{
    outputs, schema, CompileErrorKind, EditErrorKind, Graph, InputSpec, Local, LocalTaskResult,
    NodeId, NodeInputs, NodeOutputs, OutputSpec, Presence, RunInputs, Schema, Task, TaskContext,
};

fn empty_task(_: TaskContext<Local>, _: NodeInputs<Local>) -> LocalTaskResult {
    Ok(NodeOutputs::empty())
}

fn source_with_outputs(graph: &mut Graph<Local>, name: &str, outputs: Vec<OutputSpec>) -> NodeId {
    graph
        .add_sync(name, Schema::new(vec![], outputs), empty_task)
        .expect("test schema is valid")
}

fn many_sink(graph: &mut Graph<Local>, name: &str, presence: Presence) -> NodeId {
    let input = match presence {
        Presence::Required => InputSpec::required_many::<u32>("items"),
        Presence::Optional => InputSpec::optional_many::<u32>("items"),
    };
    graph
        .add_sync(name, Schema::new(vec![input], vec![]), empty_task)
        .expect("test schema is valid")
}

fn assert_edit_kind<T>(result: Result<T, slot_graph::EditError>, expected: EditErrorKind) {
    match result {
        Err(error) => assert_eq!(error.kind, expected),
        Ok(_) => panic!("edit unexpectedly succeeded"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn collect_into_collects_all_exact_type_outputs_in_caller_then_schema_order() {
    let mut graph = Graph::<Local>::new();
    let first = graph
        .add_sync(
            "first",
        schema! { () -> ("first_a": u32, "ignored": String, "packed": Vec<u32>, "first_b": u32) },
        |_, _| {
            Ok(outputs! {
                "first_a" => 10_u32,
                "ignored" => String::from("not an item"),
                "packed" => vec![100_u32, 101_u32],
                "first_b" => 11_u32,
                })
            },
        )
        .unwrap();
    let second = graph
        .add_sync("second", schema! { () -> ("second": u32) }, |_, _| {
            Ok(outputs! { "second" => 20_u32 })
        })
        .unwrap();
    // It has the exact type, but collection must never scan a node that was
    // not explicitly supplied by the caller.
    graph
        .add_sync(
            "unrelated_camera",
            schema! { () -> ("camera_resource": u32) },
            |_, _| Ok(outputs! { "camera_resource" => 999_u32 }),
        )
        .unwrap();
    // `auto_collect` is false by default. Explicit collection must not depend
    // on, or mutate, that convenience setting.
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(
                vec![InputSpec::required_many::<u32>("items")],
                vec![OutputSpec::new::<Vec<u32>>("ordered")],
            ),
            |_, inputs| {
                let items = inputs.many::<u32>("items")?;
                Ok(outputs! { "ordered" => items.iter().map(|item| **item).collect::<Vec<_>>() })
            },
        )
        .unwrap();
    let created = graph
        .collect_into([second, first], sink.input("items"))
        .unwrap();
    assert_eq!(created.len(), 3, "every compatible output is collected");
    assert!(created.windows(2).all(|pair| pair[0] != pair[1]));

    graph.set_active(sink, true).unwrap();
    let ordered = graph.output::<Vec<u32>>(sink, "ordered").unwrap();
    let version = graph
        .compile()
        .expect("the required Many input has three collected producers");
    let report = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(
        report.output(ordered).unwrap().as_slice(),
        &[20, 10, 11],
        "caller source order, then source schema order; Vec is not flattened and camera stays unconnected"
    );
}

#[test]
#[ignore = "implementation pending"]
fn collect_into_preserves_existing_bindings_and_repeated_sources_add_nothing() {
    let mut graph = Graph::<Local>::new();
    let first = graph
        .add_sync("first", schema! { () -> ("a": u32, "b": u32) }, |_, _| {
            Ok(outputs! { "a" => 10_u32, "b" => 20_u32 })
        })
        .unwrap();
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(
                vec![InputSpec::required_many::<u32>("items")],
                vec![OutputSpec::new::<Vec<u32>>("ordered")],
            ),
            |_, inputs| {
                Ok(outputs! {
                    "ordered" => inputs.many::<u32>("items")?.iter().map(|item| **item).collect::<Vec<_>>()
                })
            },
        )
        .unwrap();
    let input = graph.input::<u32>(sink, "items").unwrap();
    let existing = graph.connect(first.output("b"), input).unwrap();

    let created = graph.collect_into([first, first], input).unwrap();
    assert_eq!(
        created.len(),
        1,
        "existing pair and repeated source are skipped"
    );
    assert_ne!(
        created[0], existing,
        "only the previously unbound output is new"
    );
    assert!(
        graph.collect_into([first], input).unwrap().is_empty(),
        "a second collection is idempotent"
    );
    graph.set_active(sink, true).unwrap();
    let ordered = graph.output::<Vec<u32>>(sink, "ordered").unwrap();
    let report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(
        report.output(ordered).unwrap().as_slice(),
        &[20, 10],
        "the existing b edge remains first and the new a edge appends"
    );
}

#[test]
#[ignore = "implementation pending"]
fn collect_into_only_targets_the_requested_many_input() {
    let mut graph = Graph::<Local>::new();
    let source = source_with_outputs(&mut graph, "source", vec![OutputSpec::new::<u32>("value")]);
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(
                vec![
                    InputSpec::required_many::<u32>("chosen"),
                    InputSpec::required_many::<u32>("other"),
                ],
                vec![],
            ),
            empty_task,
        )
        .unwrap();
    let chosen = graph.input::<u32>(sink, "chosen").unwrap();

    assert_eq!(graph.collect_into([source], chosen).unwrap().len(), 1);
    graph.set_active(sink, true).unwrap();
    match graph.compile() {
        Err(error) => assert_eq!(error.kind, CompileErrorKind::MissingRequiredInput),
        Ok(_) => panic!("collection must not bind a different target slot"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn no_matches_leave_required_unsatisfied_but_allow_optional_many() {
    let mut required_graph = Graph::<Local>::new();
    let text = source_with_outputs(
        &mut required_graph,
        "text",
        vec![OutputSpec::new::<String>("text")],
    );
    let required = many_sink(&mut required_graph, "required", Presence::Required);
    let required_input = required_graph.input::<u32>(required, "items").unwrap();
    assert!(required_graph
        .collect_into([text], required_input)
        .unwrap()
        .is_empty());
    required_graph.set_active(required, true).unwrap();
    match required_graph.compile() {
        Err(error) => assert_eq!(error.kind, CompileErrorKind::MissingRequiredInput),
        Ok(_) => panic!("an empty required collection is checked at compile"),
    }

    let mut optional_graph = Graph::<Local>::new();
    let text = source_with_outputs(
        &mut optional_graph,
        "text",
        vec![OutputSpec::new::<String>("text")],
    );
    let optional = many_sink(&mut optional_graph, "optional", Presence::Optional);
    let optional_input = optional_graph.input::<u32>(optional, "items").unwrap();
    assert!(optional_graph
        .collect_into([text], optional_input)
        .unwrap()
        .is_empty());
    optional_graph.set_active(optional, true).unwrap();
    optional_graph
        .compile()
        .expect("an optional Many input accepts no matches");
}

#[test]
#[ignore = "implementation pending"]
fn collect_into_rejects_a_one_input_even_when_there_is_one_source() {
    let mut graph = Graph::<Local>::new();
    let source = source_with_outputs(&mut graph, "source", vec![OutputSpec::new::<u32>("value")]);
    let sink = graph
        .add_sync(
            "sink",
            Schema::new(vec![InputSpec::required_one::<u32>("value")], vec![]),
            empty_task,
        )
        .unwrap();
    let input = graph.input::<u32>(sink, "value").unwrap();

    assert_edit_kind(
        graph.collect_into([source], input),
        EditErrorKind::ExpectedManyInput,
    );
}

#[test]
#[ignore = "implementation pending"]
fn collect_into_validates_every_source_and_rolls_back_on_a_late_foreign_node() {
    let mut graph = Graph::<Local>::new();
    let local = source_with_outputs(&mut graph, "local", vec![OutputSpec::new::<u32>("value")]);
    let sink = many_sink(&mut graph, "sink", Presence::Required);
    let input = graph.input::<u32>(sink, "items").unwrap();

    let mut foreign_graph = Graph::<Local>::new();
    let foreign = source_with_outputs(
        &mut foreign_graph,
        "foreign",
        vec![OutputSpec::new::<u32>("value")],
    );

    assert_edit_kind(
        graph.collect_into([local, foreign], input),
        EditErrorKind::ForeignHandle,
    );
    graph
        .connect(local.output("value"), input)
        .expect("a failed batch must not have added the local edge");
}

#[test]
#[ignore = "implementation pending"]
fn collect_into_rejects_an_exposed_target_even_for_an_empty_source_list() {
    let mut graph = Graph::<Local>::new();
    let sink = many_sink(&mut graph, "sink", Presence::Optional);
    let input = graph.input::<u32>(sink, "items").unwrap();
    graph.expose_input::<u32>(input).unwrap();

    assert_edit_kind(
        graph.collect_into(std::iter::empty::<NodeId>(), input),
        EditErrorKind::InputSourceConflict,
    );
}

#[test]
#[ignore = "implementation pending"]
fn collect_into_rejects_stale_sources_and_stale_targets_without_partial_edits() {
    let mut graph = Graph::<Local>::new();
    let valid = source_with_outputs(&mut graph, "valid", vec![OutputSpec::new::<u32>("value")]);
    let stale = source_with_outputs(&mut graph, "stale", vec![OutputSpec::new::<u32>("value")]);
    let sink = many_sink(&mut graph, "sink", Presence::Required);
    let old_input = graph.input::<u32>(sink, "items").unwrap();
    graph.remove_node(stale).unwrap();

    assert_edit_kind(
        graph.collect_into([valid, stale], old_input),
        EditErrorKind::StaleNodeId,
    );
    graph
        .connect(valid.output("value"), old_input)
        .expect("a stale late source must roll back the preceding valid source");

    graph
        .replace_schema(
            sink,
            Schema::new(vec![InputSpec::required_many::<u32>("items")], vec![]),
            Task::<Local>::sync(empty_task),
        )
        .unwrap();
    assert_edit_kind(
        graph.collect_into([valid], old_input),
        EditErrorKind::StaleSlotHandle,
    );
}
