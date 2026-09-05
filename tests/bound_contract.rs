//! Contract tests for pre-bound Schema layouts and typed task keys.
//!
//! The runtime remains an API skeleton, so these executable specifications are
//! ignored. They deliberately use only public APIs and become normal tests as
//! each part of the implementation is completed.

use futures_lite::future::block_on;
use slot_graph::{
    EditError, EditErrorKind, ExecuteError, Graph, InputSpec, Local, NodeError, NodeErrorKind,
    NodeOutputs, NodeStatus, OutputAccessErrorKind, OutputSpec, RunInputs, Schema, Shared, Task,
};

#[derive(Debug, PartialEq, Eq)]
struct NonClonePayload(u32);

fn assert_edit_kind<T>(result: Result<T, EditError>, expected: EditErrorKind) {
    match result {
        Err(error) => assert_eq!(error.kind, expected),
        Ok(_) => panic!("operation unexpectedly succeeded"),
    }
}

fn assert_failed_with(
    result: Result<slot_graph::RunReport<Local>, ExecuteError<Local>>,
    node: slot_graph::NodeId,
    kind: NodeErrorKind,
) {
    match result {
        Err(ExecuteError::Failed(report)) => {
            assert_eq!(report.status(node), Some(NodeStatus::Failed));
            assert_eq!(report.failures().count(), 1);
            assert_eq!(report.failures().next().unwrap().error.kind, kind);
        }
        _ => panic!("run unexpectedly succeeded or was cancelled"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn bound_lookup_reports_unknown_names_wrong_types_and_invalid_shape() {
    let mut editable = Schema::new(
        vec![InputSpec::required_one::<u32>("count")],
        vec![OutputSpec::new::<String>("label")],
    );
    let bound = editable.clone().bind();
    editable.inputs[0].name = "renamed_after_bind".to_owned();

    assert!(bound.input::<u32>("count").is_ok());
    assert_edit_kind(
        bound.input::<u32>("missing"),
        EditErrorKind::UnknownSlotName,
    );
    assert_edit_kind(bound.input::<String>("count"), EditErrorKind::TypeMismatch);
    assert_edit_kind(bound.output::<u32>("label"), EditErrorKind::TypeMismatch);

    let invalid = Schema::new(
        vec![InputSpec::required_one::<u32>("still_rejected")],
        vec![
            OutputSpec::new::<u32>("duplicated"),
            OutputSpec::new::<u32>("duplicated"),
        ],
    )
    .bind();
    assert_edit_kind(
        invalid.input::<u32>("still_rejected"),
        EditErrorKind::InvalidSchema,
    );
}

#[test]
#[ignore = "implementation pending"]
fn invalid_bound_schema_is_rejected_by_registration_and_atomic_replacement() {
    let invalid = Schema::new(
        vec![],
        vec![
            OutputSpec::new::<u32>("same"),
            OutputSpec::new::<u32>("same"),
        ],
    )
    .bind();
    let mut graph = Graph::<Local>::new();
    assert_edit_kind(
        graph.add_sync("node", invalid.clone(), |_, _| Ok(NodeOutputs::empty())),
        EditErrorKind::InvalidSchema,
    );
    assert_edit_kind(
        graph.add_async("node", invalid.clone(), |_, _| async {
            Ok(NodeOutputs::empty())
        }),
        EditErrorKind::InvalidSchema,
    );

    // Failed registrations must not reserve the node name or create a node.
    let valid = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
    let value = valid.output::<u32>("value").unwrap();
    let node = graph
        .add_sync("node", valid, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(value, 7_u32);
            Ok(outputs)
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<u32>(node, "value").unwrap();
    let before = graph.compile().unwrap();
    assert_edit_kind(
        graph.replace_schema(
            node,
            invalid,
            Task::<Local>::sync(|_, _| Ok(NodeOutputs::empty())),
        ),
        EditErrorKind::InvalidSchema,
    );
    let after = graph.compile().unwrap();
    // Both the old version and the current declaration keep the original task,
    // layout, active target, and graph-facing output handle after failure.
    for version in [before, after] {
        let report = block_on(version.execute(RunInputs::new())).unwrap();
        assert_eq!(**report.output(output).unwrap(), 7);
    }
}

#[test]
#[ignore = "implementation pending"]
fn bound_keys_read_required_optional_and_many_inputs() {
    let producer_schema = Schema::new(
        vec![],
        vec![
            OutputSpec::new::<u32>("required"),
            OutputSpec::new::<u32>("many"),
        ],
    )
    .bind();
    let required_output = producer_schema.output::<u32>("required").unwrap();
    let many_output = producer_schema.output::<u32>("many").unwrap();
    let consumer_schema = Schema::new(
        vec![
            InputSpec::required_one::<u32>("required"),
            InputSpec::optional_one::<u32>("optional"),
            InputSpec::required_many::<u32>("many"),
            InputSpec::optional_many::<u32>("missing_many"),
        ],
        vec![],
    )
    .bind();
    let required = consumer_schema.input::<u32>("required").unwrap();
    let optional = consumer_schema.input::<u32>("optional").unwrap();
    let many = consumer_schema.input::<u32>("many").unwrap();
    let missing_many = consumer_schema.input::<u32>("missing_many").unwrap();

    let mut graph = Graph::<Local>::new();
    let producer = graph
        .add_sync("producer", producer_schema, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(required_output, 2_u32);
            outputs.insert_key(many_output, 5_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let consumer = graph
        .add_sync("consumer", consumer_schema, move |_, inputs| {
            assert_eq!(*inputs.required_key(required)?, 2);
            assert_eq!(inputs.optional_key(optional)?.as_deref(), None);
            assert_eq!(
                inputs
                    .many_key(many)?
                    .iter()
                    .map(|value| **value)
                    .collect::<Vec<_>>(),
                vec![5]
            );
            assert!(inputs.many_key(missing_many)?.is_empty());
            Ok::<_, NodeError<Local>>(NodeOutputs::empty())
        })
        .unwrap();
    graph
        .connect(producer.output("required"), consumer.input("required"))
        .unwrap();
    graph
        .connect(producer.output("many"), consumer.input("many"))
        .unwrap();
    graph.set_active(consumer, true).unwrap();

    block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
}

#[test]
#[ignore = "implementation pending"]
fn named_and_keyed_duplicate_outputs_fail_as_one_atomic_commit() {
    for keyed_first in [false, true] {
        let schema = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
        let value = schema.output::<u32>("value").unwrap();
        let mut graph = Graph::<Local>::new();
        let producer = graph
            .add_sync("producer", schema, move |_, _| {
                let mut outputs = NodeOutputs::new();
                if keyed_first {
                    outputs.insert_key(value, 2_u32);
                    outputs.insert("value", 1_u32);
                } else {
                    outputs.insert("value", 1_u32);
                    outputs.insert_key(value, 2_u32);
                }
                Ok::<_, NodeError<Local>>(outputs)
            })
            .unwrap();
        let consumer = graph
            .add_sync(
                "consumer",
                Schema::new(vec![InputSpec::required_one::<u32>("value")], vec![]),
                |_, _| Ok::<_, NodeError<Local>>(NodeOutputs::empty()),
            )
            .unwrap();
        let output = graph.output::<u32>(producer, "value").unwrap();
        graph
            .connect(producer.output("value"), consumer.input("value"))
            .unwrap();
        graph.set_active(producer, true).unwrap();
        graph.set_active(consumer, true).unwrap();

        match block_on(graph.compile().unwrap().execute(RunInputs::new())) {
            Err(ExecuteError::Failed(report)) => {
                assert_eq!(report.status(producer), Some(NodeStatus::Failed));
                assert_eq!(report.status(consumer), Some(NodeStatus::Blocked));
                assert_eq!(report.failures().count(), 1);
                assert_eq!(
                    report.failures().next().unwrap().error.kind,
                    NodeErrorKind::InvalidOutputs
                );
                assert!(matches!(
                    report.output(output),
                    Err(error) if error.kind == OutputAccessErrorKind::OutputUnavailable
                ));
            }
            _ => panic!("duplicate outputs must not publish a partial value"),
        }
    }
}

#[test]
#[ignore = "implementation pending"]
fn a_key_from_another_bound_layout_is_an_invalid_input() {
    let expected = Schema::new(vec![InputSpec::optional_one::<u32>("value")], vec![]).bind();
    let foreign = Schema::new(vec![InputSpec::optional_one::<u32>("value")], vec![]).bind();
    let foreign_key = foreign.input::<u32>("value").unwrap();

    let mut graph = Graph::<Local>::new();
    let target = graph
        .add_sync("target", expected, move |_, inputs| {
            let _ = inputs.optional_key(foreign_key)?;
            Ok::<_, NodeError<Local>>(NodeOutputs::empty())
        })
        .unwrap();
    graph.set_active(target, true).unwrap();

    assert_failed_with(
        block_on(graph.compile().unwrap().execute(RunInputs::new())),
        target,
        NodeErrorKind::InvalidInputs,
    );
}

#[test]
#[ignore = "implementation pending"]
fn a_key_with_the_right_layout_but_wrong_accessor_shape_is_an_invalid_input() {
    let schema = Schema::new(vec![InputSpec::optional_one::<u32>("value")], vec![]).bind();
    let value = schema.input::<u32>("value").unwrap();
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync("node", schema, move |_, inputs| {
            // Layout validation succeeds; only the Required-vs-Optional shape
            // mismatch makes this task input invalid.
            let _ = inputs.required_key(value)?;
            Ok::<_, NodeError<Local>>(NodeOutputs::empty())
        })
        .unwrap();
    graph.set_active(node, true).unwrap();

    assert_failed_with(
        block_on(graph.compile().unwrap().execute(RunInputs::new())),
        node,
        NodeErrorKind::InvalidInputs,
    );
}

#[test]
#[ignore = "implementation pending"]
fn a_key_from_another_bound_layout_is_an_invalid_output_without_consumers() {
    let registered = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
    let foreign = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
    let foreign_key = foreign.output::<u32>("value").unwrap();
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync("producer", registered, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(foreign_key, 1_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    graph.set_active(node, true).unwrap();

    assert_failed_with(
        block_on(graph.compile().unwrap().execute(RunInputs::new())),
        node,
        NodeErrorKind::InvalidOutputs,
    );
}

#[test]
#[ignore = "implementation pending"]
fn a_cloned_bound_schema_deliberately_shares_its_key_layout() {
    let source_schema = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
    let source_value = source_schema.output::<u32>("value").unwrap();
    let bound = Schema::new(
        vec![InputSpec::required_one::<u32>("value")],
        vec![OutputSpec::new::<u32>("answer")],
    )
    .bind();
    let input = bound.input::<u32>("value").unwrap();
    let answer = bound.output::<u32>("answer").unwrap();

    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_sync("source", source_schema, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(source_value, 41_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let first = graph
        .add_sync("first", bound.clone(), move |_, inputs| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(answer, *inputs.required_key(input)? + 1);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let second = graph
        .add_sync("second", bound, move |_, inputs| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(answer, *inputs.required_key(input)? + 2);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    for target in [first, second] {
        graph
            .connect(source.output("value"), target.input("value"))
            .unwrap();
        graph.set_active(target, true).unwrap();
    }

    let first_output = graph.output::<u32>(first, "answer").unwrap();
    let second_output = graph.output::<u32>(second, "answer").unwrap();
    let report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(**report.output(first_output).unwrap(), 42);
    assert_eq!(**report.output(second_output).unwrap(), 43);
}

#[test]
#[ignore = "implementation pending"]
fn keyed_shared_values_forward_a_non_clone_payload_without_rewrapping_it() {
    let source_schema =
        Schema::new(vec![], vec![OutputSpec::new::<NonClonePayload>("payload")]).bind();
    let source_output = source_schema.output::<NonClonePayload>("payload").unwrap();
    let sink_schema = Schema::new(
        vec![InputSpec::required_one::<NonClonePayload>("payload")],
        vec![OutputSpec::new::<NonClonePayload>("payload")],
    )
    .bind();
    let sink_input = sink_schema.input::<NonClonePayload>("payload").unwrap();
    let sink_output = sink_schema.output::<NonClonePayload>("payload").unwrap();
    let original = Shared::<NonClonePayload, Local>::new(NonClonePayload(9));

    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_sync("source", source_schema, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_shared_key(source_output, original.clone());
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let sink = graph
        .add_sync("sink", sink_schema, move |_, inputs| {
            let value = inputs.required_key(sink_input)?;
            let mut outputs = NodeOutputs::new();
            outputs.insert_shared_key(sink_output, value);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let output = graph.output::<NonClonePayload>(sink, "payload").unwrap();
    graph
        .connect(source.output("payload"), sink.input("payload"))
        .unwrap();
    graph.set_active(sink, true).unwrap();

    let report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(report.output(output).unwrap().as_ref(), &NonClonePayload(9));
}

#[test]
#[ignore = "implementation pending"]
fn rebound_or_reordered_schema_rejects_an_old_key() {
    let original = Schema::new(
        vec![InputSpec::required_one::<u32>("first")],
        vec![OutputSpec::new::<u32>("out")],
    )
    .bind();
    let old = original.input::<u32>("first").unwrap();
    let rebound = Schema::new(
        vec![
            InputSpec::optional_one::<u32>("second"),
            InputSpec::required_one::<u32>("first"),
        ],
        vec![OutputSpec::new::<u32>("out")],
    )
    .bind();
    let first = rebound.input::<u32>("first").unwrap();
    let output = rebound.output::<u32>("out").unwrap();
    let source_schema = Schema::new(vec![], vec![OutputSpec::new::<u32>("first")]).bind();
    let source_output = source_schema.output::<u32>("first").unwrap();

    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_sync("source", source_schema, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(source_output, 1_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let node = graph
        .add_sync("node", rebound, move |_, inputs| {
            let _ = inputs.required_key(first)?;
            let _ = inputs.required_key(old)?;
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(output, 1_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    graph
        .connect(source.output("first"), node.input("first"))
        .unwrap();
    graph.set_active(node, true).unwrap();

    assert_failed_with(
        block_on(graph.compile().unwrap().execute(RunInputs::new())),
        node,
        NodeErrorKind::InvalidInputs,
    );
}

#[test]
#[ignore = "implementation pending"]
fn old_versions_keep_their_layout_while_replace_schema_uses_a_new_one() {
    let old_schema = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
    let old_value = old_schema.output::<u32>("value").unwrap();
    let new_schema = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
    let new_value = new_schema.output::<u32>("value").unwrap();
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync("node", old_schema, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(old_value, 1_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let old_output = graph.output::<u32>(node, "value").unwrap();
    let old_version = graph.compile().unwrap();

    graph
        .replace_schema(
            node,
            new_schema,
            Task::<Local>::sync(move |_, _| {
                let mut outputs = NodeOutputs::new();
                outputs.insert_key(new_value, 2_u32);
                Ok::<_, NodeError<Local>>(outputs)
            }),
        )
        .unwrap();
    let new_version = graph.compile().unwrap();
    let new_output = graph.output::<u32>(node, "value").unwrap();

    let old_report = block_on(old_version.execute(RunInputs::new())).unwrap();
    let new_report = block_on(new_version.execute(RunInputs::new())).unwrap();
    assert_eq!(**old_report.output(old_output).unwrap(), 1);
    assert_eq!(**new_report.output(new_output).unwrap(), 2);
}

#[test]
#[ignore = "implementation pending"]
fn replacing_with_a_bound_schema_clone_keeps_keys_compatible_but_slot_handles_stale() {
    let bound = Schema::new(
        vec![InputSpec::optional_one::<u32>("input")],
        vec![OutputSpec::new::<u32>("value")],
    )
    .bind();
    let value = bound.output::<u32>("value").unwrap();
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync("node", bound.clone(), move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(value, 1_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let stale_slot = graph.output::<u32>(node, "value").unwrap();
    graph
        .replace_schema(
            node,
            bound,
            Task::<Local>::sync(move |_, _| {
                let mut outputs = NodeOutputs::new();
                outputs.insert_key(value, 2_u32);
                Ok::<_, NodeError<Local>>(outputs)
            }),
        )
        .unwrap();
    block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    let current_input = graph.input::<u32>(node, "input").unwrap();

    assert_edit_kind(
        graph.connect(stale_slot, current_input),
        EditErrorKind::StaleSlotHandle,
    );
}

#[test]
#[ignore = "implementation pending"]
fn replace_sync_and_replace_async_preserve_the_existing_bound_schema() {
    let bound = Schema::new(vec![], vec![OutputSpec::new::<u32>("value")]).bind();
    let value = bound.output::<u32>("value").unwrap();
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync("node", bound, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(value, 1_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<u32>(node, "value").unwrap();
    let v1 = graph.compile().unwrap();
    graph
        .replace_sync(node, move |_, _| {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(value, 2_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let v2 = graph.compile().unwrap();
    graph
        .replace_async(node, move |_, _| async move {
            let mut outputs = NodeOutputs::new();
            outputs.insert_key(value, 3_u32);
            Ok::<_, NodeError<Local>>(outputs)
        })
        .unwrap();
    let v3 = graph.compile().unwrap();

    let r1 = block_on(v1.execute(RunInputs::new())).unwrap();
    let r2 = block_on(v2.execute(RunInputs::new())).unwrap();
    let r3 = block_on(v3.execute(RunInputs::new())).unwrap();
    assert_eq!(**r1.output(output).unwrap(), 1);
    assert_eq!(**r2.output(output).unwrap(), 2);
    assert_eq!(**r3.output(output).unwrap(), 3);
}
