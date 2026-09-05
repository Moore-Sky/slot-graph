//! Runtime contracts for the public Slot Graph API.
//!
//! These tests verify scheduling, validation, cancellation, reporting, and
//! runner reuse through the public API.

use std::{
    cell::Cell,
    future::Future,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
};

use slot_graph::{
    outputs, schema, ExecuteError, Graph, Local, NodeStatus, OutputAccessErrorKind, RunInputs,
};

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

fn noop_waker() -> Waker {
    Waker::from(Arc::new(NoopWake))
}

#[test]
fn required_run_input_fails_before_any_task_starts() {
    let started = Rc::new(Cell::new(0));
    let mut graph = Graph::<Local>::new();
    let counter = Rc::clone(&started);
    let node = graph
        .add_sync(
            "consume",
            schema! { ("value": u32) -> () },
            move |_task, _inputs| {
                counter.set(counter.get() + 1);
                Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
            },
        )
        .unwrap();
    graph.expose_input::<u32>(node.input("value")).unwrap();
    graph.set_active(node, true).unwrap();
    let version = graph.compile().unwrap();

    assert!(matches!(
        futures_lite::future::block_on(version.execute(RunInputs::new())),
        Err(ExecuteError::Start(_))
    ));
    assert_eq!(started.get(), 0);
}

#[test]
fn optional_run_input_may_be_absent() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "optional",
            schema! { ("value": Optional<u32>) -> ("present": bool) },
            |_task, inputs| {
                Ok::<_, slot_graph::NodeError<Local>>(
                    outputs! { "present" => inputs.optional::<u32>("value")?.is_some() },
                )
            },
        )
        .unwrap();
    graph.expose_input::<u32>(node.input("value")).unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<bool>(node, "present").unwrap();
    let report =
        futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();

    assert!(!*report.output(output).unwrap().as_ref());
}

#[test]
fn many_run_input_preserves_extend_order() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "collect",
            schema! { ("items": Many<u32>) -> ("items": Vec<u32>) },
            |_task, inputs| {
                let values = inputs.many::<u32>("items")?;
                Ok::<_, slot_graph::NodeError<Local>>(outputs! { "items" => values.into_iter().map(|value| *value).collect::<Vec<_>>() })
            },
        )
        .unwrap();
    let input = graph.expose_input::<u32>(node.input("items")).unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<Vec<u32>>(node, "items").unwrap();
    let version = graph.compile().unwrap();
    let mut inputs = RunInputs::<Local>::new();
    inputs.extend(input, [3_u32, 1, 4]).unwrap();

    let report = futures_lite::future::block_on(version.execute(inputs)).unwrap();
    assert_eq!(report.output(output).unwrap().as_ref(), &[3, 1, 4]);
}

#[test]
fn foreign_run_input_is_rejected_at_start() {
    let mut first = Graph::<Local>::new();
    let first_node = first
        .add_sync(
            "first",
            schema! { ("value": u32) -> () },
            |_task, _inputs| Ok::<_, slot_graph::NodeError<Local>>(outputs! {}),
        )
        .unwrap();
    first
        .expose_input::<u32>(first_node.input("value"))
        .unwrap();
    first.set_active(first_node, true).unwrap();
    let version = first.compile().unwrap();

    let mut second = Graph::<Local>::new();
    let second_node = second
        .add_sync(
            "second",
            schema! { ("value": u32) -> () },
            |_task, _inputs| Ok::<_, slot_graph::NodeError<Local>>(outputs! {}),
        )
        .unwrap();
    let foreign = second
        .expose_input::<u32>(second_node.input("value"))
        .unwrap();
    let mut inputs = RunInputs::<Local>::new();
    inputs.insert(foreign, 9_u32).unwrap();

    assert!(matches!(
        futures_lite::future::block_on(version.execute(inputs)),
        Err(ExecuteError::Start(_))
    ));
}

#[test]
fn missing_declared_output_fails_without_a_partial_commit() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "broken",
            schema! { () -> ("a": u32, "b": u32) },
            |_task, _inputs| Ok::<_, slot_graph::NodeError<Local>>(outputs! { "a" => 1_u32 }),
        )
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<u32>(node, "a").unwrap();

    match futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => match report.output(output) {
            Err(error) => assert_eq!(error.kind, OutputAccessErrorKind::OutputUnavailable),
            Ok(_) => panic!("a failed target must not retain a partial output"),
        },
        _ => panic!("expected failed report"),
    }
}

#[test]
fn unknown_declared_output_fails_without_a_partial_commit() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync("broken", schema! { () -> ("a": u32) }, |_task, _inputs| {
            Ok::<_, slot_graph::NodeError<Local>>(outputs! { "a" => 1_u32, "extra" => 2_u32 })
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<u32>(node, "a").unwrap();

    match futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => match report.output(output) {
            Err(error) => assert_eq!(error.kind, OutputAccessErrorKind::OutputUnavailable),
            Ok(_) => panic!("a failed target must not retain a partial output"),
        },
        _ => panic!("expected failed report"),
    }
}

#[test]
fn report_keeps_successful_active_target_output() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "produce",
            schema! { () -> ("value": String) },
            |_task, _inputs| {
                Ok::<_, slot_graph::NodeError<Local>>(outputs! { "value" => String::from("ready") })
            },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<String>(node, "value").unwrap();

    let report =
        futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();
    assert_eq!(report.output(output).unwrap().as_ref(), "ready");
}

#[test]
fn take_output_transfers_only_the_requested_target_value() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_sync(
            "produce",
            schema! { () -> ("a": u32, "b": u32) },
            |_task, _inputs| {
                Ok::<_, slot_graph::NodeError<Local>>(outputs! { "a" => 1_u32, "b" => 2_u32 })
            },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();
    let a = graph.output::<u32>(node, "a").unwrap();
    let b = graph.output::<u32>(node, "b").unwrap();
    let mut report =
        futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())).unwrap();

    assert_eq!(*report.take_output(a).unwrap(), 1);
    match report.output(a) {
        Err(error) => assert_eq!(error.kind, OutputAccessErrorKind::OutputTaken),
        Ok(_) => panic!("taken output must no longer be available from the report"),
    }
    assert_eq!(**report.output(b).unwrap(), 2);
}

#[test]
fn failure_blocks_only_its_downstream_and_keeps_independent_work() {
    let mut graph = Graph::<Local>::new();
    let failed = graph
        .add_sync(
            "failed",
            schema! { () -> ("value": u32) },
            |_task, _inputs| {
                Err::<slot_graph::NodeOutputs<Local>, _>(slot_graph::NodeError::<Local>::user(
                    std::io::Error::new(std::io::ErrorKind::Other, "boom"),
                ))
            },
        )
        .unwrap();
    let blocked = graph
        .add_sync(
            "blocked",
            schema! { ("value": u32) -> () },
            |_task, _inputs| Ok::<_, slot_graph::NodeError<Local>>(outputs! {}),
        )
        .unwrap();
    let independent = graph
        .add_sync("independent", schema! { () -> () }, |_task, _inputs| {
            Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
        })
        .unwrap();
    graph
        .connect(failed.output("value"), blocked.input("value"))
        .unwrap();
    graph.set_active(blocked, true).unwrap();
    graph.set_active(independent, true).unwrap();

    match futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => {
            assert_eq!(report.status(failed), Some(NodeStatus::Failed));
            assert_eq!(report.status(blocked), Some(NodeStatus::Blocked));
            assert_eq!(report.status(independent), Some(NodeStatus::Succeeded));
        }
        _ => panic!("expected failed report"),
    }
}

#[test]
fn independent_failures_are_all_retained_in_the_report() {
    let mut graph = Graph::<Local>::new();
    let first = graph
        .add_sync("first", schema! { () -> () }, |_task, _inputs| {
            Err::<slot_graph::NodeOutputs<Local>, _>(slot_graph::NodeError::<Local>::user(
                std::io::Error::new(std::io::ErrorKind::Other, "one"),
            ))
        })
        .unwrap();
    let second = graph
        .add_sync("second", schema! { () -> () }, |_task, _inputs| {
            Err::<slot_graph::NodeOutputs<Local>, _>(slot_graph::NodeError::<Local>::user(
                std::io::Error::new(std::io::ErrorKind::Other, "two"),
            ))
        })
        .unwrap();
    graph.set_active(first, true).unwrap();
    graph.set_active(second, true).unwrap();

    match futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => assert_eq!(report.failures().len(), 2),
        _ => panic!("expected failed report"),
    }
}

#[test]
fn cancellation_before_first_poll_starts_no_task() {
    let started = Arc::new(AtomicUsize::new(0));
    let mut graph = Graph::<Local>::new();
    let count = Arc::clone(&started);
    let node = graph
        .add_async("wait", schema! { () -> () }, move |_task, _inputs| {
            let count = Arc::clone(&count);
            count.fetch_add(1, Ordering::SeqCst);
            async move {
                std::future::pending::<()>().await;
                Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let run = graph.compile().unwrap().start(RunInputs::new()).unwrap();
    run.control().cancel();

    assert!(matches!(
        futures_lite::future::block_on(run),
        Err(ExecuteError::Cancelled(_))
    ));
    assert_eq!(started.load(Ordering::SeqCst), 0);
}

#[test]
fn abort_drops_pending_future_and_returns_cancelled_report() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let started = Arc::new(AtomicUsize::new(0));
    let mut graph = Graph::<Local>::new();
    let drops = Arc::clone(&dropped);
    let starts = Arc::clone(&started);
    let node = graph
        .add_async("pending", schema! { () -> () }, move |_task, _inputs| {
            struct PendingDrop(Arc<AtomicUsize>);
            impl Drop for PendingDrop {
                fn drop(&mut self) {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
            starts.fetch_add(1, Ordering::SeqCst);
            let guard = PendingDrop(Arc::clone(&drops));
            async move {
                let _guard = guard;
                std::future::pending::<()>().await;
                Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let run = graph.compile().unwrap().start(RunInputs::new()).unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);

    for _ in 0..32 {
        assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
        if started.load(Ordering::SeqCst) == 1 {
            break;
        }
    }
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "pending task never started"
    );
    control.abort();

    assert!(matches!(
        futures_lite::future::block_on(run),
        Err(ExecuteError::Cancelled(_))
    ));
    assert_eq!(dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn runner_reuses_storage_across_completed_runs() {
    let calls = Rc::new(Cell::new(0));
    let mut graph = Graph::<Local>::new();
    let count = Rc::clone(&calls);
    let node = graph
        .add_sync("frame", schema! { () -> () }, move |_task, _inputs| {
            count.set(count.get() + 1);
            Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let version = graph.compile().unwrap();
    let mut runner = version.runner();

    futures_lite::future::block_on(runner.execute(RunInputs::new())).unwrap();
    futures_lite::future::block_on(runner.execute(RunInputs::new())).unwrap();
    assert_eq!(calls.get(), 2);
}

#[test]
fn runner_run_exposes_control_and_is_reusable_after_abort() {
    let mut graph = Graph::<Local>::new();
    let node = graph
        .add_async(
            "pending",
            schema! { () -> () },
            |_task, _inputs| async move {
                std::future::pending::<()>().await;
                Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
            },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();
    let version = graph.compile().unwrap();
    let mut runner = version.runner();

    let run = runner.start(RunInputs::new()).unwrap();
    run.control().abort();
    assert!(matches!(
        futures_lite::future::block_on(run),
        Err(ExecuteError::Cancelled(_))
    ));
    let second = runner.start(RunInputs::new()).unwrap();
    second.control().abort();
    assert!(matches!(
        futures_lite::future::block_on(second),
        Err(ExecuteError::Cancelled(_))
    ));
}

#[test]
fn active_target_output_survives_an_independent_failure() {
    let mut graph = Graph::<Local>::new();
    let good = graph
        .add_sync(
            "good",
            schema! { () -> ("value": u32) },
            |_task, _inputs| Ok::<_, slot_graph::NodeError<Local>>(outputs! { "value" => 7_u32 }),
        )
        .unwrap();
    let bad = graph
        .add_sync("bad", schema! { () -> () }, |_task, _inputs| {
            Err::<slot_graph::NodeOutputs<Local>, _>(slot_graph::NodeError::<Local>::user(
                std::io::Error::new(std::io::ErrorKind::Other, "bad"),
            ))
        })
        .unwrap();
    graph.set_active(good, true).unwrap();
    graph.set_active(bad, true).unwrap();
    let output = graph.output::<u32>(good, "value").unwrap();

    match futures_lite::future::block_on(graph.compile().unwrap().execute(RunInputs::new())) {
        Err(ExecuteError::Failed(report)) => assert_eq!(**report.output(output).unwrap(), 7),
        _ => panic!("expected failed report"),
    }
}

#[test]
fn pending_async_node_does_not_unlock_its_successor_when_manually_polled() {
    let successor_started = Rc::new(Cell::new(0));
    let mut graph = Graph::<Local>::new();
    let source = graph
        .add_async(
            "source",
            schema! { () -> ("value": u32) },
            |_task, _inputs| async move {
                std::future::pending::<()>().await;
                Ok::<_, slot_graph::NodeError<Local>>(outputs! { "value" => 1_u32 })
            },
        )
        .unwrap();
    let count = Rc::clone(&successor_started);
    let successor = graph
        .add_sync(
            "successor",
            schema! { ("value": u32) -> () },
            move |_task, _inputs| {
                count.set(count.get() + 1);
                Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
            },
        )
        .unwrap();
    graph
        .connect(source.output("value"), successor.input("value"))
        .unwrap();
    graph.set_active(successor, true).unwrap();
    let mut run = Box::pin(graph.compile().unwrap().start(RunInputs::new()).unwrap());
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);

    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    assert_eq!(successor_started.get(), 0);
}

#[test]
fn commit_before_cancel_keeps_the_already_committed_target_output() {
    let mut graph = Graph::<Local>::new();
    let complete = graph
        .add_sync(
            "complete",
            schema! { () -> ("value": u32) },
            |_task, _inputs| Ok::<_, slot_graph::NodeError<Local>>(outputs! { "value" => 42_u32 }),
        )
        .unwrap();
    let pending_started = Rc::new(Cell::new(0));
    let started = Rc::clone(&pending_started);
    let pending = graph
        .add_async(
            "pending",
            schema! { ("value": u32) -> () },
            move |task, _inputs| {
                started.set(started.get() + 1);
                async move {
                    task.cancellation().cancelled().await;
                    Ok::<_, slot_graph::NodeError<Local>>(outputs! {})
                }
            },
        )
        .unwrap();
    graph.set_active(complete, true).unwrap();
    graph.set_active(pending, true).unwrap();
    graph
        .connect(complete.output("value"), pending.input("value"))
        .unwrap();
    let output = graph.output::<u32>(complete, "value").unwrap();
    let run = graph.compile().unwrap().start(RunInputs::new()).unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let waker = noop_waker();
    let mut context = Context::from_waker(&waker);

    for _ in 0..32 {
        assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
        if pending_started.get() == 1 {
            break;
        }
    }
    assert_eq!(pending_started.get(), 1, "pending task never started");
    control.cancel();

    match futures_lite::future::block_on(run) {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(**report.output(output).unwrap(), 42);
        }
        _ => panic!("expected cancelled report"),
    }
}
