//! Contracts for dispatching ready nodes through an external scheduler.
//!
//! A dispatcher is deliberately a scheduling boundary, not a second graph
//! runtime. These tests verify orchestration and independently scheduled Ready
//! nodes together.

use std::{
    collections::VecDeque,
    future::Future,
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Barrier, Mutex,
    },
    task::{Context, Poll, Wake, Waker},
    thread,
    time::Duration,
};

use futures_lite::future::block_on;
use slot_graph::{
    outputs, schema, DispatchError, ExecuteError, Graph, NodeDispatcher, NodeErrorKind, NodeId,
    NodeJob, NodeStatus, OutputAccessErrorKind, RunControl, RunInputs, SendMode,
};

const WAIT: Duration = Duration::from_secs(2);

/// A deliberately small two-or-more-thread adapter used only by contracts.
/// It represents the shape an async-runtime adapter will have: queue one
/// ready node as an independent Future and return immediately.
#[derive(Clone, Default)]
struct ThreadDispatcher {
    submitted: Arc<Mutex<Vec<NodeId>>>,
}

impl ThreadDispatcher {
    fn submitted(&self) -> Vec<NodeId> {
        self.submitted.lock().unwrap().clone()
    }
}

impl NodeDispatcher<SendMode> for ThreadDispatcher {
    fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
        self.submitted.lock().unwrap().push(job.node_id());
        thread::Builder::new()
            .name("slot-graph-contract-worker".into())
            .spawn(move || block_on(job))
            .map(|_| ())
            .map_err(DispatchError::with_source)
    }
}

fn wait_for(rx: &Receiver<()>, message: &str) {
    rx.recv_timeout(WAIT).expect(message);
}

fn send_all(tx: &Sender<()>, count: usize) {
    for _ in 0..count {
        tx.send(()).expect("workers must still be waiting");
    }
}

fn send_signal(tx: &Arc<Mutex<Sender<()>>>) {
    tx.lock()
        .unwrap()
        .send(())
        .expect("signal receiver must still be waiting");
}

#[test]
fn independent_send_nodes_are_submitted_as_distinct_overlapping_jobs() {
    let (started_tx, started_rx) = mpsc::channel();
    let started_tx = Arc::new(Mutex::new(started_tx));
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let mut graph = Graph::<SendMode>::new();

    for name in ["a", "b"] {
        let started = started_tx.clone();
        let release = Arc::clone(&release_rx);
        let node = graph
            .add_sync(name, schema! { () -> ("done": ()) }, move |_, _| {
                send_signal(&started);
                release.lock().unwrap().recv_timeout(WAIT).unwrap();
                Ok(outputs! { "done" => () })
            })
            .unwrap();
        graph.set_active(node, true).unwrap();
    }
    drop(started_tx);

    let dispatcher = ThreadDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .execute_on(RunInputs::new(), dispatcher.clone());
    let driver = thread::spawn(move || block_on(run));

    // A serial GraphRun would enter only one task and time out here.  Both
    // independent jobs must reach their task bodies before either is released.
    wait_for(&started_rx, "first ready node was not dispatched");
    wait_for(&started_rx, "second independent node was not dispatched");
    assert_eq!(dispatcher.submitted().len(), 2);
    send_all(&release_tx, 2);
    assert!(driver.join().unwrap().is_ok());
}

#[test]
fn dependent_node_is_not_submitted_before_its_upstream_output_commits() {
    let (source_started_tx, source_started_rx) = mpsc::channel();
    let source_started_tx = Arc::new(Mutex::new(source_started_tx));
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let mut graph = Graph::<SendMode>::new();
    let started = source_started_tx.clone();
    let release = Arc::clone(&release_rx);
    let source = graph
        .add_sync("source", schema! { () -> ("value": u32) }, move |_, _| {
            send_signal(&started);
            release.lock().unwrap().recv_timeout(WAIT).unwrap();
            Ok(outputs! { "value" => 7_u32 })
        })
        .unwrap();
    let sink = graph
        .add_sync("sink", schema! { ("value": u32) -> () }, |_, inputs| {
            assert_eq!(*inputs.required::<u32>("value")?, 7);
            Ok(outputs! {})
        })
        .unwrap();
    graph.connect_nodes(source, sink).unwrap();
    graph.set_active(sink, true).unwrap();
    let dispatcher = ThreadDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .execute_on(RunInputs::new(), dispatcher.clone());
    let driver = thread::spawn(move || block_on(run));

    wait_for(&source_started_rx, "source was not dispatched");
    assert_eq!(dispatcher.submitted(), vec![source]);
    release_tx.send(()).unwrap();
    assert!(driver.join().unwrap().is_ok());
    assert_eq!(dispatcher.submitted(), vec![source, sink]);
}

#[test]
fn a_node_with_multiple_ready_edges_is_dispatched_once() {
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut graph = Graph::<SendMode>::new();
    let source = graph
        .add_sync("source", schema! { () -> ("a": u32, "b": u32) }, |_, _| {
            Ok(outputs! { "a" => 1_u32, "b" => 2_u32 })
        })
        .unwrap();
    let called = Arc::clone(&invocations);
    let sink = graph
        .add_sync(
            "sink",
            schema! { ("items": Many<u32>) -> () },
            move |_, inputs| {
                called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(inputs.many::<u32>("items")?.len(), 2);
                Ok(outputs! {})
            },
        )
        .unwrap();
    graph.collect_into([source], sink.input("items")).unwrap();
    graph.set_active(sink, true).unwrap();

    let dispatcher = ThreadDispatcher::default();
    assert!(block_on(
        graph
            .compile()
            .unwrap()
            .execute_on(RunInputs::new(), dispatcher.clone())
    )
    .is_ok());
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(
        dispatcher
            .submitted()
            .into_iter()
            .filter(|submitted| *submitted == sink)
            .count(),
        1
    );
}

#[test]
fn many_inputs_keep_edge_order_when_producers_complete_out_of_order() {
    let (first_release_tx, first_release_rx) = mpsc::channel();
    let first_release_rx = Arc::new(Mutex::new(first_release_rx));
    let (second_committed_tx, second_committed_rx) = mpsc::channel();
    let second_committed_tx = Arc::new(Mutex::new(second_committed_tx));
    let mut graph = Graph::<SendMode>::new();
    let first = graph
        .add_sync("first", schema! { () -> ("value": u32) }, move |_, _| {
            first_release_rx.lock().unwrap().recv_timeout(WAIT).unwrap();
            Ok(outputs! { "value" => 10_u32 })
        })
        .unwrap();
    let second = graph
        .add_sync("second", schema! { () -> ("value": u32) }, move |_, _| {
            Ok(outputs! { "value" => 20_u32 })
        })
        .unwrap();
    let observer = graph
        .add_sync(
            "second_observer",
            schema! { ("value": u32) -> () },
            move |_, inputs| {
                assert_eq!(*inputs.required::<u32>("value")?, 20);
                send_signal(&second_committed_tx);
                Ok(outputs! {})
            },
        )
        .unwrap();
    let sink = graph
        .add_sync("sink", schema! { ("items": Many<u32>) -> ("ordered": Vec<u32>) }, |_, inputs| {
            Ok(outputs! { "ordered" => inputs.many::<u32>("items")?.iter().map(|item| **item).collect::<Vec<_>>() })
        })
        .unwrap();
    graph
        .collect_into([first, second], sink.input("items"))
        .unwrap();
    graph.connect_nodes(second, observer).unwrap();
    graph.set_active(sink, true).unwrap();
    graph.set_active(observer, true).unwrap();
    let output = graph.output::<Vec<u32>>(sink, "ordered").unwrap();
    let run = graph
        .compile()
        .unwrap()
        .execute_on(RunInputs::new(), ThreadDispatcher::default());
    let driver = thread::spawn(move || block_on(run));

    wait_for(
        &second_committed_rx,
        "second producer did not commit before the first producer was released",
    );
    first_release_tx.send(()).unwrap();
    let report = driver.join().unwrap().unwrap();
    assert_eq!(report.output(output).unwrap().as_slice(), &[10, 20]);
}

#[test]
fn dispatch_rejection_fails_only_that_node_and_independent_work_can_finish() {
    #[derive(Clone)]
    struct RejectOne {
        rejected: NodeId,
        submitted: Arc<Mutex<Vec<NodeId>>>,
    }
    impl RejectOne {
        fn submitted(&self) -> Vec<NodeId> {
            self.submitted.lock().unwrap().clone()
        }
    }
    impl NodeDispatcher<SendMode> for RejectOne {
        fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
            if self.rejected == job.node_id() {
                return Err(DispatchError::new("pool is shutting down"));
            }
            self.submitted.lock().unwrap().push(job.node_id());
            thread::spawn(move || block_on(job));
            Ok(())
        }
    }

    let mut graph = Graph::<SendMode>::new();
    let rejected = graph
        .add_sync("rejected", schema! { () -> ("value": u32) }, |_, _| {
            Ok(outputs! { "value" => 1_u32 })
        })
        .unwrap();
    let downstream = graph
        .add_sync("downstream", schema! { ("value": u32) -> () }, |_, _| {
            Ok(outputs! {})
        })
        .unwrap();
    let completed = graph
        .add_sync("completed", schema! { () -> () }, |_, _| Ok(outputs! {}))
        .unwrap();
    graph.set_active(rejected, true).unwrap();
    graph.connect_nodes(rejected, downstream).unwrap();
    graph.set_active(downstream, true).unwrap();
    graph.set_active(completed, true).unwrap();
    let dispatcher = RejectOne {
        rejected,
        submitted: Arc::new(Mutex::new(Vec::new())),
    };

    match block_on(
        graph
            .compile()
            .unwrap()
            .execute_on(RunInputs::new(), dispatcher.clone()),
    ) {
        Err(ExecuteError::Failed(report)) => {
            assert_eq!(report.status(rejected), Some(NodeStatus::Failed));
            assert_eq!(report.status(downstream), Some(NodeStatus::Blocked));
            assert_eq!(report.status(completed), Some(NodeStatus::Succeeded));
            assert!(report.failures().any(|failure| {
                failure.node == rejected && failure.error.kind == NodeErrorKind::Dispatch
            }));
            let failure = report
                .failures()
                .find(|failure| failure.node == rejected)
                .unwrap();
            assert_eq!(
                std::error::Error::source(&failure.error)
                    .unwrap()
                    .to_string(),
                "pool is shutting down"
            );
            assert!(!dispatcher.submitted().contains(&downstream));
        }
        _ => panic!("a dispatcher rejection is a structured node failure"),
    }
}

#[test]
fn cancellation_wins_before_a_dispatched_result_can_commit() {
    let (started_tx, started_rx) = mpsc::channel();
    let started_tx = Arc::new(Mutex::new(started_tx));
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Arc::new(Mutex::new(release_rx));
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync("node", schema! { () -> ("value": u32) }, move |_, _| {
            send_signal(&started_tx);
            release_rx.lock().unwrap().recv_timeout(WAIT).unwrap();
            Ok(outputs! { "value" => 42_u32 })
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<u32>(node, "value").unwrap();
    let run = graph
        .compile()
        .unwrap()
        .execute_on(RunInputs::new(), ThreadDispatcher::default());
    let control = run.control();
    let (done_tx, done_rx) = mpsc::channel();
    thread::spawn(move || done_tx.send(block_on(run)).unwrap());

    wait_for(&started_rx, "dispatched task did not start");
    control.cancel();
    release_tx.send(()).unwrap();

    match done_rx
        .recv_timeout(WAIT)
        .expect("cancelled run did not finish")
    {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(report.status(node), Some(NodeStatus::Cancelled));
            assert!(matches!(
                report.output(output),
                Err(error) if error.kind == OutputAccessErrorKind::OutputUnavailable
            ));
        }
        _ => panic!("cancellation must win before the result commit point"),
    }
}

/// Forces cancellation precisely after the child Future has returned Pending
/// and before NodeJob can register the worker's waker with its cancel state.
/// A NodeJob must arrange one catch-up poll in that race; otherwise the worker
/// sleeps forever because cancellation drained the waiter list too early.
#[test]
fn cancellation_between_child_pending_and_node_job_waiter_registration_is_not_lost() {
    struct CancelThenPending {
        control: Arc<Mutex<Option<RunControl<SendMode>>>>,
        cancelled: bool,
    }

    impl Future for CancelThenPending {
        type Output = slot_graph::SendTaskResult;

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.cancelled {
                self.cancelled = true;
                self.control
                    .lock()
                    .unwrap()
                    .as_ref()
                    .expect("run control is installed before dispatch")
                    .cancel();
                Poll::Pending
            } else {
                Poll::Ready(Ok(outputs! {}))
            }
        }
    }

    let controls = Arc::new(Mutex::new(None));
    let future_controls = Arc::clone(&controls);
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_async("race", schema! { () -> () }, move |_task, _inputs| {
            CancelThenPending {
                control: Arc::clone(&future_controls),
                cancelled: false,
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();

    let dispatcher = HoldingDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .start_on(RunInputs::new(), dispatcher.clone())
        .unwrap();
    *controls.lock().unwrap() = Some(run.control());
    let mut run = Box::pin(run);
    let driver_waker = Waker::from(Arc::new(NoopWake));
    let mut driver_context = Context::from_waker(&driver_waker);

    assert!(matches!(
        run.as_mut().poll(&mut driver_context),
        Poll::Pending
    ));
    let mut job = Box::pin(dispatcher.pop());
    let job_wake = Arc::new(CountWake::default());
    let job_waker = Waker::from(Arc::clone(&job_wake));
    let mut job_context = Context::from_waker(&job_waker);

    assert!(matches!(job.as_mut().poll(&mut job_context), Poll::Pending));
    assert_eq!(
        job_wake.count(),
        1,
        "NodeJob must schedule one catch-up poll after the raced cancellation"
    );
    assert!(matches!(
        job.as_mut().poll(&mut job_context),
        Poll::Ready(())
    ));

    match block_on(run) {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(report.status(node), Some(NodeStatus::Cancelled));
        }
        _ => panic!("the raced dispatched node must finish as cancelled"),
    }
}

#[test]
fn cooperative_cancel_catch_up_does_not_spin_and_later_abort_still_wakes() {
    struct CancelThenStayPending {
        control: Arc<Mutex<Option<RunControl<SendMode>>>>,
        cancelled: bool,
    }

    impl Future for CancelThenStayPending {
        type Output = slot_graph::SendTaskResult;

        fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.cancelled {
                self.cancelled = true;
                self.control
                    .lock()
                    .unwrap()
                    .as_ref()
                    .expect("run control is installed before dispatch")
                    .cancel();
            }
            Poll::Pending
        }
    }

    let controls = Arc::new(Mutex::new(None));
    let future_controls = Arc::clone(&controls);
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_async(
            "unresponsive_cancel_race",
            schema! { () -> () },
            move |_task, _inputs| CancelThenStayPending {
                control: Arc::clone(&future_controls),
                cancelled: false,
            },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();

    let dispatcher = HoldingDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .start_on(RunInputs::new(), dispatcher.clone())
        .unwrap();
    let control = run.control();
    *controls.lock().unwrap() = Some(control.clone());
    let mut run = Box::pin(run);
    let driver_waker = Waker::from(Arc::new(NoopWake));
    let mut driver_context = Context::from_waker(&driver_waker);
    assert!(matches!(
        run.as_mut().poll(&mut driver_context),
        Poll::Pending
    ));

    let mut job = Box::pin(dispatcher.pop());
    let job_wake = Arc::new(CountWake::default());
    let job_waker = Waker::from(Arc::clone(&job_wake));
    let mut job_context = Context::from_waker(&job_waker);
    assert!(matches!(job.as_mut().poll(&mut job_context), Poll::Pending));
    assert_eq!(job_wake.count(), 1);

    assert!(matches!(job.as_mut().poll(&mut job_context), Poll::Pending));
    assert_eq!(
        job_wake.count(),
        1,
        "an unresponsive cooperatively cancelled job must not self-wake forever"
    );

    control.abort();
    assert_eq!(
        job_wake.count(),
        2,
        "abort must retain a waiter after cooperative cancellation"
    );
    assert!(matches!(
        job.as_mut().poll(&mut job_context),
        Poll::Ready(())
    ));
    assert!(matches!(block_on(run), Err(ExecuteError::Cancelled(_))));
}

/// Uses a two-party barrier to place abort after the child Future enters its
/// first poll but before NodeJob can register the worker waker.
#[test]
fn abort_between_child_pending_and_node_job_waiter_registration_finishes_the_job() {
    struct BarrierThenPending {
        gate: Arc<Barrier>,
        dropped: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Future for BarrierThenPending {
        type Output = slot_graph::SendTaskResult;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.gate.wait();
            self.gate.wait();
            Poll::Pending
        }
    }

    impl Drop for BarrierThenPending {
        fn drop(&mut self) {
            self.dropped
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    let gate = Arc::new(Barrier::new(2));
    let dropped = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let future_gate = Arc::clone(&gate);
    let future_dropped = Arc::clone(&dropped);
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_async("abort_race", schema! { () -> () }, move |_task, _inputs| {
            BarrierThenPending {
                gate: Arc::clone(&future_gate),
                dropped: Arc::clone(&future_dropped),
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();

    let dispatcher = HoldingDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .start_on(RunInputs::new(), dispatcher.clone())
        .unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let driver_waker = Waker::from(Arc::new(NoopWake));
    let mut driver_context = Context::from_waker(&driver_waker);
    assert!(matches!(
        run.as_mut().poll(&mut driver_context),
        Poll::Pending
    ));

    let job = dispatcher.pop();
    let (poll_tx, poll_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut job = Box::pin(job);
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        poll_tx.send(job.as_mut().poll(&mut context)).unwrap();
    });

    gate.wait();
    control.abort();
    gate.wait();
    assert!(matches!(
        poll_rx
            .recv_timeout(WAIT)
            .expect("the raced abort must finish NodeJob"),
        Poll::Ready(())
    ));
    worker.join().unwrap();
    assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));

    match block_on(run) {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(report.status(node), Some(NodeStatus::Cancelled));
        }
        _ => panic!("the raced abort must cancel the dispatched node"),
    }
}

#[test]
fn cancellation_does_not_hide_invalid_dispatched_outputs() {
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync("invalid", schema! { () -> ("value": u32) }, |_, _| {
            Ok(outputs! {})
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let dispatcher = HoldingDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .start_on(RunInputs::new(), dispatcher.clone())
        .unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);

    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    let job = dispatcher.pop();
    block_on(job);
    control.cancel();

    match block_on(run) {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(report.status(node), Some(NodeStatus::Failed));
            assert!(report.failures().any(|failure| {
                failure.node == node && failure.error.kind == NodeErrorKind::InvalidOutputs
            }));
        }
        _ => panic!("output validation must happen before the cancellation decision"),
    }
}

#[test]
fn abort_waits_for_an_accepted_queued_job_to_acknowledge_drop() {
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let called = Arc::clone(&invocations);
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync("queued", schema! { () -> () }, move |_, _| {
            called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(outputs! {})
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let dispatcher = HoldingDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .start_on(RunInputs::new(), dispatcher.clone())
        .unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);

    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    let queued = dispatcher.pop();
    control.abort();
    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    drop(queued);

    match block_on(run) {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(report.status(node), Some(NodeStatus::Cancelled));
        }
        _ => panic!("abort must finish after the queued job acknowledges drop"),
    }
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn cooperative_cancel_prevents_a_queued_job_from_invoking_its_factory() {
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let called = Arc::clone(&invocations);
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync("queued", schema! { () -> () }, move |_, _| {
            called.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(outputs! {})
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let dispatcher = HoldingDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .start_on(RunInputs::new(), dispatcher.clone())
        .unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);

    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    let queued = dispatcher.pop();
    control.cancel();
    block_on(queued);

    match block_on(run) {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(report.status(node), Some(NodeStatus::Cancelled));
        }
        _ => panic!("cancelled queued work must acknowledge without starting"),
    }
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn externally_dispatched_commit_before_cancel_keeps_target_output() {
    let mut graph = Graph::<SendMode>::new();
    let producer = graph
        .add_sync("producer", schema! { () -> ("value": u32) }, |_, _| {
            Ok(outputs! { "value" => 42_u32 })
        })
        .unwrap();
    let queued = graph
        .add_sync("queued", schema! { () -> () }, |_, _| Ok(outputs! {}))
        .unwrap();
    graph.set_active(producer, true).unwrap();
    graph.set_active(queued, true).unwrap();
    let output = graph.output::<u32>(producer, "value").unwrap();
    let dispatcher = HoldingDispatcher::default();
    let run = graph
        .compile()
        .unwrap()
        .start_on(RunInputs::new(), dispatcher.clone())
        .unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);

    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    let producer_job = dispatcher.pop();
    let queued_job = dispatcher.pop();
    assert_eq!(producer_job.node_id(), producer);
    assert_eq!(queued_job.node_id(), queued);

    block_on(producer_job);
    assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    control.cancel();
    drop(queued_job);

    match block_on(run) {
        Err(ExecuteError::Cancelled(report)) => {
            assert_eq!(report.status(producer), Some(NodeStatus::Succeeded));
            assert_eq!(report.status(queued), Some(NodeStatus::Cancelled));
            assert_eq!(**report.output(output).unwrap(), 42);
        }
        _ => panic!("cancellation must not retract an earlier dispatched commit"),
    }
}

#[cfg(panic = "unwind")]
#[test]
fn user_panic_is_a_node_panic_not_a_dispatch_failure() {
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync(
            "panic",
            schema! { () -> () },
            |_, _| -> slot_graph::SendTaskResult { panic!("user task panic") },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();

    match block_on(
        graph
            .compile()
            .unwrap()
            .execute_on(RunInputs::new(), ThreadDispatcher::default()),
    ) {
        Err(ExecuteError::Failed(report)) => {
            let failure = report
                .failures()
                .find(|failure| failure.node == node)
                .unwrap();
            assert_eq!(failure.error.kind, NodeErrorKind::Panic);
        }
        _ => panic!("panic must be reported as a node failure"),
    }
}

#[test]
fn an_accepted_but_dropped_job_becomes_a_dispatch_failure() {
    #[derive(Clone, Copy)]
    struct DropAccepted;
    impl NodeDispatcher<SendMode> for DropAccepted {
        fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
            drop(job);
            Ok(())
        }
    }

    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync("lost", schema! { () -> () }, |_, _| Ok(outputs! {}))
        .unwrap();
    graph.set_active(node, true).unwrap();
    let run = graph
        .compile()
        .unwrap()
        .execute_on(RunInputs::new(), DropAccepted);
    let (done_tx, done_rx) = mpsc::channel();
    let driver = thread::spawn(move || {
        done_tx.send(block_on(run)).unwrap();
    });
    let result = done_rx
        .recv_timeout(WAIT)
        .expect("dropping an accepted job must wake and finish GraphRun");
    driver.join().unwrap();

    match result {
        Err(ExecuteError::Failed(report)) => {
            let failure = report
                .failures()
                .find(|failure| failure.node == node)
                .unwrap();
            assert_eq!(failure.error.kind, NodeErrorKind::Dispatch);
        }
        _ => panic!("a lost accepted job is a structured node failure"),
    }
}

#[cfg(panic = "unwind")]
#[test]
fn a_dispatcher_panic_is_contained_as_a_dispatch_failure() {
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync("node", schema! { () -> () }, |_, _| Ok(outputs! {}))
        .unwrap();
    graph.set_active(node, true).unwrap();
    let dispatcher =
        |_job: NodeJob<SendMode>| -> Result<(), DispatchError> { panic!("dispatcher panic") };

    let result = catch_unwind(AssertUnwindSafe(|| {
        block_on(
            graph
                .compile()
                .unwrap()
                .execute_on(RunInputs::new(), dispatcher),
        )
    }))
    .expect("dispatcher panic must not unwind through GraphRun");
    match result {
        Err(ExecuteError::Failed(report)) => {
            let failure = report
                .failures()
                .find(|failure| failure.node == node)
                .unwrap();
            assert_eq!(failure.error.kind, NodeErrorKind::Dispatch);
        }
        _ => panic!("dispatcher panic is a structured node failure"),
    }
}

#[derive(Clone, Default)]
struct HoldingDispatcher {
    jobs: Arc<Mutex<VecDeque<NodeJob<SendMode>>>>,
}

impl HoldingDispatcher {
    fn pop(&self) -> NodeJob<SendMode> {
        self.jobs
            .lock()
            .unwrap()
            .pop_front()
            .expect("a Ready job must have been submitted")
    }
}

impl NodeDispatcher<SendMode> for HoldingDispatcher {
    fn dispatch(&self, job: NodeJob<SendMode>) -> Result<(), DispatchError> {
        self.jobs.lock().unwrap().push_back(job);
        Ok(())
    }
}

struct NoopWake;
impl Wake for NoopWake {
    fn wake(self: Arc<Self>) {}
}

#[derive(Default)]
struct CountWake(std::sync::atomic::AtomicUsize);

impl CountWake {
    fn count(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn a_late_job_from_a_dropped_runner_run_cannot_modify_reused_storage() {
    let invocations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let called = Arc::clone(&invocations);
    let mut graph = Graph::<SendMode>::new();
    let node = graph
        .add_sync("frame", schema! { () -> ("value": usize) }, move |_, _| {
            let value = called.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Ok(outputs! { "value" => value })
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let output = graph.output::<usize>(node, "value").unwrap();
    let dispatcher = HoldingDispatcher::default();
    let mut runner = graph.compile().unwrap().runner_on(dispatcher.clone());
    let waker = Waker::from(Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);

    let mut first = Box::pin(runner.start(RunInputs::new()).unwrap());
    assert!(matches!(first.as_mut().poll(&mut context), Poll::Pending));
    drop(first);

    let mut second = Box::pin(runner.start(RunInputs::new()).unwrap());
    assert!(matches!(second.as_mut().poll(&mut context), Poll::Pending));
    let stale = dispatcher.pop();
    let current = dispatcher.pop();

    // The stale job notices its retired run generation and must not invoke the
    // task or publish into storage now owned by `second`.
    block_on(stale);
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 0);
    block_on(current);
    assert_eq!(invocations.load(std::sync::atomic::Ordering::SeqCst), 1);

    let report = block_on(second).unwrap();
    assert_eq!(**report.output(output).unwrap(), 1);
}
