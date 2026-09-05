//! Output commit, report retention, and run-isolation contracts.
//!
//! The scheduler is not implemented yet, so these tests remain ignored while
//! still compiling against the public API. Remove an ignore marker only when
//! the corresponding behavior is implemented end to end.

use std::{
    cell::Cell,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use futures_lite::future::block_on;
use slot_graph::{
    outputs, schema, ExecuteError, Graph, Local, NodeError, NodeErrorKind, NodeOutputs, NodeStatus,
    OutputAccessErrorKind, RunInputs, Task,
};

#[test]
#[ignore = "implementation pending"]
fn duplicate_outputs_fail_atomically_and_block_the_consumer() {
    let mut graph = Graph::<Local>::new();
    let producer = graph
        .add_sync(
            "producer",
            schema! { () -> ("value": u32) },
            |_task, _inputs| {
                Ok::<_, NodeError<Local>>(outputs! {
                    "value" => 1_u32,
                    "value" => 2_u32,
                })
            },
        )
        .unwrap();
    let consumer = graph
        .add_sync(
            "consumer",
            schema! { ("value": u32) -> () },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! {}),
        )
        .unwrap();
    graph
        .connect(producer.output("value"), consumer.input("value"))
        .unwrap();
    graph.set_active(consumer, true).unwrap();

    match block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => {
            assert_eq!(report.status(producer), Some(NodeStatus::Failed));
            assert_eq!(report.status(consumer), Some(NodeStatus::Blocked));
            assert_eq!(report.failures().len(), 1);
            assert!(matches!(
                report.failures().next().unwrap().error.kind,
                NodeErrorKind::InvalidOutputs
            ));
        }
        _ => panic!("duplicate output must fail the run"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn wrongly_typed_output_fails_atomically_and_blocks_the_consumer() {
    let mut graph = Graph::<Local>::new();
    let producer = graph
        .add_sync(
            "producer",
            schema! { () -> ("value": u32) },
            |_task, _inputs| {
                Ok::<_, NodeError<Local>>(outputs! { "value" => String::from("wrong") })
            },
        )
        .unwrap();
    let consumer = graph
        .add_sync(
            "consumer",
            schema! { ("value": u32) -> () },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! {}),
        )
        .unwrap();
    graph
        .connect(producer.output("value"), consumer.input("value"))
        .unwrap();
    graph.set_active(consumer, true).unwrap();

    match block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => {
            assert_eq!(report.status(producer), Some(NodeStatus::Failed));
            assert_eq!(report.status(consumer), Some(NodeStatus::Blocked));
            assert!(matches!(
                report.failures().next().unwrap().error.kind,
                NodeErrorKind::InvalidOutputs
            ));
        }
        _ => panic!("wrongly typed output must fail the run"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn complete_multi_output_producer_executes_once_for_one_node_pair() {
    let calls = Rc::new(Cell::new(0));
    let mut graph = Graph::<Local>::new();
    let count = Rc::clone(&calls);
    let producer = graph
        .add_sync(
            "producer",
            schema! { () -> ("left": u32, "right": u32) },
            move |_task, _inputs| {
                count.set(count.get() + 1);
                Ok::<_, NodeError<Local>>(outputs! { "left" => 20_u32, "right" => 22_u32 })
            },
        )
        .unwrap();
    let consumer_calls = Rc::new(Cell::new(0));
    let consumer_count = Rc::clone(&consumer_calls);
    let consumer = graph
        .add_sync(
            "consumer",
            schema! { ("left": u32, "right": u32) -> ("sum": u32) },
            move |_task, inputs| {
                consumer_count.set(consumer_count.get() + 1);
                let left = inputs.required::<u32>("left")?;
                let right = inputs.required::<u32>("right")?;
                Ok::<_, NodeError<Local>>(outputs! { "sum" => *left + *right })
            },
        )
        .unwrap();
    graph
        .connect(producer.output("left"), consumer.input("left"))
        .unwrap();
    graph
        .connect(producer.output("right"), consumer.input("right"))
        .unwrap();
    graph.set_active(consumer, true).unwrap();
    let sum = graph.output::<u32>(consumer, "sum").unwrap();

    let report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(calls.get(), 1);
    assert_eq!(consumer_calls.get(), 1);
    assert_eq!(**report.output(sum).unwrap(), 42);
}

#[test]
#[ignore = "implementation pending"]
fn report_distinguishes_non_target_foreign_and_stale_outputs() {
    let mut graph = Graph::<Local>::new();
    let selected = graph
        .add_sync(
            "selected",
            schema! { () -> ("value": u32) },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! { "value" => 1_u32 }),
        )
        .unwrap();
    let non_target = graph
        .add_sync(
            "non_target",
            schema! { () -> ("value": u32) },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! { "value" => 2_u32 }),
        )
        .unwrap();
    graph.set_active(selected, true).unwrap();
    let non_target_output = graph.output::<u32>(non_target, "value").unwrap();
    let version = graph.compile().unwrap();

    let mut foreign_graph = Graph::<Local>::new();
    let foreign_node = foreign_graph
        .add_sync(
            "foreign",
            schema! { () -> ("value": u32) },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! { "value" => 3_u32 }),
        )
        .unwrap();
    let foreign_output = foreign_graph.output::<u32>(foreign_node, "value").unwrap();

    graph
        .replace_schema(
            selected,
            schema! { () -> ("value": u32) },
            Task::<Local>::sync(|_task, _inputs| {
                Ok::<_, NodeError<Local>>(outputs! { "value" => 4_u32 })
            }),
        )
        .unwrap();
    let stale_output = graph.output::<u32>(selected, "value").unwrap();
    let report = block_on(version.execute(RunInputs::new())).unwrap();

    match report.output(non_target_output) {
        Err(error) => assert!(matches!(error.kind, OutputAccessErrorKind::NotCollected)),
        Ok(_) => panic!("non-target output must not be retained"),
    }
    match report.output(foreign_output) {
        Err(error) => assert!(matches!(error.kind, OutputAccessErrorKind::ForeignHandle)),
        Ok(_) => panic!("foreign output must be rejected"),
    }
    match report.output(stale_output) {
        Err(error) => assert!(matches!(error.kind, OutputAccessErrorKind::StaleSlotHandle)),
        Ok(_) => panic!("new schema handle must be stale for the old version"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn failed_target_output_is_unavailable_in_the_failure_report() {
    let mut graph = Graph::<Local>::new();
    let failed = graph
        .add_sync(
            "failed",
            schema! { () -> ("value": u32) },
            |_task, _inputs| {
                Err::<NodeOutputs<Local>, _>(NodeError::user(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "failed",
                )))
            },
        )
        .unwrap();
    graph.set_active(failed, true).unwrap();
    let output = graph.output::<u32>(failed, "value").unwrap();

    match block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => match report.output(output) {
            Err(error) => assert!(matches!(
                error.kind,
                OutputAccessErrorKind::OutputUnavailable
            )),
            Ok(_) => panic!("failed target cannot expose a new output"),
        },
        _ => panic!("task error must return a failed report"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn report_strongly_retains_output_until_taken_value_is_dropped() {
    struct Probe(Arc<AtomicUsize>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    let drops = Arc::new(AtomicUsize::new(0));
    let mut graph = Graph::<Local>::new();
    let task_drops = Arc::clone(&drops);
    let node = graph
        .add_sync(
            "produce",
            schema! { () -> ("value": Probe) },
            move |_task, _inputs| {
                Ok::<_, NodeError<Local>>(outputs! { "value" => Probe(Arc::clone(&task_drops)) })
            },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<Probe>(node, "value").unwrap();

    let mut report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    let taken = report.take_output(output).unwrap();
    match report.output(output) {
        Err(error) => assert!(matches!(error.kind, OutputAccessErrorKind::OutputTaken)),
        Ok(_) => panic!("taken output must not remain readable from the report"),
    }
    drop(report);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(taken);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

#[cfg(panic = "unwind")]
#[test]
#[ignore = "implementation pending"]
fn panic_becomes_node_failure_while_independent_target_completes() {
    let mut graph = Graph::<Local>::new();
    let panic_node = graph
        .add_sync(
            "panic",
            schema! { () -> () },
            |_task, _inputs| -> Result<NodeOutputs<Local>, NodeError<Local>> {
                panic!("intentional contract panic")
            },
        )
        .unwrap();
    let good = graph
        .add_sync(
            "good",
            schema! { () -> ("value": u32) },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! { "value" => 7_u32 }),
        )
        .unwrap();
    graph.set_active(panic_node, true).unwrap();
    graph.set_active(good, true).unwrap();
    let output = graph.output::<u32>(good, "value").unwrap();

    match block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => {
            assert_eq!(report.status(panic_node), Some(NodeStatus::Failed));
            assert_eq!(report.status(good), Some(NodeStatus::Succeeded));
            assert!(matches!(
                report.failures().next().unwrap().error.kind,
                NodeErrorKind::Panic
            ));
            assert_eq!(**report.output(output).unwrap(), 7);
        }
        _ => panic!("unwind panic must become a failure report"),
    }
}

#[test]
#[ignore = "implementation pending"]
fn many_fanin_uses_connection_order_in_the_real_output() {
    let mut graph = Graph::<Local>::new();
    let first = graph
        .add_sync(
            "first",
            schema! { () -> ("item": u32) },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! { "item" => 3_u32 }),
        )
        .unwrap();
    let second = graph
        .add_sync(
            "second",
            schema! { () -> ("item": u32) },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! { "item" => 1_u32 }),
        )
        .unwrap();
    let third = graph
        .add_sync(
            "third",
            schema! { () -> ("item": u32) },
            |_task, _inputs| Ok::<_, NodeError<Local>>(outputs! { "item" => 4_u32 }),
        )
        .unwrap();
    let merge = graph
        .add_sync(
            "merge",
            schema! { ("items": Many<u32>) -> ("order": Vec<u32>) },
            |_task, inputs| {
                let values = inputs.many::<u32>("items")?;
                Ok::<_, NodeError<Local>>(outputs! {
                    "order" => values.into_iter().map(|value| *value).collect::<Vec<_>>()
                })
            },
        )
        .unwrap();
    graph
        .connect(first.output("item"), merge.input("items"))
        .unwrap();
    graph
        .connect(second.output("item"), merge.input("items"))
        .unwrap();
    graph
        .connect(third.output("item"), merge.input("items"))
        .unwrap();
    graph.set_active(merge, true).unwrap();
    let order = graph.output::<Vec<u32>>(merge, "order").unwrap();

    let report = block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(report.output(order).unwrap().as_ref(), &[3, 1, 4]);
}

#[test]
#[ignore = "implementation pending"]
fn repeated_runs_of_one_version_keep_output_values_isolated() {
    let next = Arc::new(AtomicUsize::new(0));
    let mut graph = Graph::<Local>::new();
    let counter = Arc::clone(&next);
    let node = graph
        .add_sync(
            "counter",
            schema! { () -> ("value": usize) },
            move |_task, _inputs| {
                let value = counter.fetch_add(1, Ordering::SeqCst) + 1;
                Ok::<_, NodeError<Local>>(outputs! { "value" => value })
            },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<usize>(node, "value").unwrap();
    let version = graph.compile().unwrap();

    let first = block_on(version.execute(RunInputs::new())).unwrap();
    let second = block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(**first.output(output).unwrap(), 1);
    assert_eq!(**second.output(output).unwrap(), 2);
}
