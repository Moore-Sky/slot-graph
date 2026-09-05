//! Async runtime contracts for the public Slot Graph API.
//!
//! Pending paths are driven with a bounded hand-written poll loop; no test
//! awaits an unresponsive future.

use std::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll, Wake, Waker},
};

use slot_graph::{outputs, schema, Cancelled, ExecuteError, Graph, Local, RunInputs};

const MAX_POLLS: usize = 32;

#[derive(Default)]
struct GateState {
    ready: bool,
    polls: usize,
    drops: usize,
    waker: Option<Waker>,
}

#[derive(Clone, Default)]
struct Gate {
    state: Rc<RefCell<GateState>>,
}

impl Gate {
    fn wait(&self) -> GateFuture {
        GateFuture { gate: self.clone() }
    }

    fn release(&self) {
        let waker = {
            let mut state = self.state.borrow_mut();
            state.ready = true;
            state.waker.clone()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn wake_last(&self) {
        if let Some(waker) = self.state.borrow().waker.clone() {
            waker.wake();
        }
    }

    fn polls(&self) -> usize {
        self.state.borrow().polls
    }

    fn drops(&self) -> usize {
        self.state.borrow().drops
    }
}

struct GateFuture {
    gate: Gate,
}

impl Future for GateFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.gate.state.borrow_mut();
        state.polls += 1;
        if state.ready {
            Poll::Ready(())
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

impl Drop for GateFuture {
    fn drop(&mut self) {
        self.gate.state.borrow_mut().drops += 1;
    }
}

struct GateOrCancellation {
    gate: GateFuture,
    cancellation: Cancelled<Local>,
}

impl Future for GateOrCancellation {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if Pin::new(&mut self.cancellation).poll(cx).is_ready() {
            Poll::Ready(())
        } else {
            Pin::new(&mut self.gate).poll(cx)
        }
    }
}

struct CountingWake(AtomicUsize);

impl CountingWake {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

impl Wake for CountingWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn counting_waker() -> (Arc<CountingWake>, Waker) {
    let wake = Arc::new(CountingWake(AtomicUsize::new(0)));
    (Arc::clone(&wake), Waker::from(wake))
}

fn poll_until_pending<F: Future>(
    mut future: Pin<&mut F>,
    context: &mut Context<'_>,
    mut condition: impl FnMut() -> bool,
) {
    for _ in 0..MAX_POLLS {
        assert!(matches!(future.as_mut().poll(context), Poll::Pending));
        if condition() {
            return;
        }
    }
    panic!("condition was not reached within {MAX_POLLS} pending polls");
}

#[test]
fn multiple_pending_gates_progress_independently() {
    let first_gate = Gate::default();
    let second_gate = Gate::default();
    let finished = Rc::new(RefCell::new(Vec::new()));
    let mut graph = Graph::<Local>::new();

    let first = {
        let gate = first_gate.clone();
        let finished = Rc::clone(&finished);
        graph
            .add_async("first", schema! { () -> () }, move |_task, _inputs| {
                let gate = gate.clone();
                let finished = Rc::clone(&finished);
                async move {
                    gate.wait().await;
                    finished.borrow_mut().push("first");
                    Ok(outputs! {})
                }
            })
            .unwrap()
    };
    let second = {
        let gate = second_gate.clone();
        let finished = Rc::clone(&finished);
        graph
            .add_async("second", schema! { () -> () }, move |_task, _inputs| {
                let gate = gate.clone();
                let finished = Rc::clone(&finished);
                async move {
                    gate.wait().await;
                    finished.borrow_mut().push("second");
                    Ok(outputs! {})
                }
            })
            .unwrap()
    };
    graph.set_active(first, true).unwrap();
    graph.set_active(second, true).unwrap();

    let mut run = Box::pin(graph.compile().unwrap().start(RunInputs::new()).unwrap());
    let (_wake, waker) = counting_waker();
    let mut context = Context::from_waker(&waker);
    poll_until_pending(run.as_mut(), &mut context, || {
        first_gate.polls() > 0 && second_gate.polls() > 0
    });

    first_gate.release();
    poll_until_pending(run.as_mut(), &mut context, || {
        finished.borrow().as_slice() == ["first"]
    });
    assert_eq!(finished.borrow().as_slice(), ["first"]);

    second_gate.release();
    futures_lite::future::block_on(run).unwrap();
    assert_eq!(finished.borrow().as_slice(), ["first", "second"]);
}

#[test]
fn duplicate_and_stale_wakes_do_not_execute_a_node_twice() {
    let gate = Gate::default();
    let sink_calls = Rc::new(RefCell::new(0_usize));
    let mut graph = Graph::<Local>::new();
    let source_gate = gate.clone();
    let source = graph
        .add_async(
            "source",
            schema! { () -> ("value": u32) },
            move |_task, _inputs| {
                let gate = source_gate.clone();
                async move {
                    gate.wait().await;
                    Ok(outputs! { "value" => 7_u32 })
                }
            },
        )
        .unwrap();
    let calls = Rc::clone(&sink_calls);
    let sink = graph
        .add_sync(
            "sink",
            schema! { ("value": u32) -> () },
            move |_task, _inputs| {
                *calls.borrow_mut() += 1;
                Ok(outputs! {})
            },
        )
        .unwrap();
    graph
        .connect(source.output("value"), sink.input("value"))
        .unwrap();
    graph.set_active(sink, true).unwrap();

    let mut run = Box::pin(graph.compile().unwrap().start(RunInputs::new()).unwrap());
    let (wake_count, waker) = counting_waker();
    let mut context = Context::from_waker(&waker);
    poll_until_pending(run.as_mut(), &mut context, || gate.polls() > 0);

    gate.release();
    gate.wake_last();
    gate.wake_last();
    assert!(wake_count.count() >= 2);
    futures_lite::future::block_on(run).unwrap();
    assert_eq!(*sink_calls.borrow(), 1);

    // The gate retains an old waker deliberately.  Waking it after completion
    // must not rerun user code.
    gate.wake_last();
    assert_eq!(*sink_calls.borrow(), 1);
}

#[test]
fn async_factory_creates_a_fresh_future_for_every_run() {
    let factories = Rc::new(RefCell::new(0_usize));
    let mut graph = Graph::<Local>::new();
    let count = Rc::clone(&factories);
    let node = graph
        .add_async("fresh", schema! { () -> () }, move |_task, _inputs| {
            *count.borrow_mut() += 1;
            async move { Ok(outputs! {}) }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let version = graph.compile().unwrap();

    futures_lite::future::block_on(version.execute(RunInputs::new())).unwrap();
    futures_lite::future::block_on(version.execute(RunInputs::new())).unwrap();
    assert_eq!(*factories.borrow(), 2);
}

#[test]
fn cancelling_one_of_two_runs_does_not_affect_the_other() {
    let gate = Gate::default();
    let factories = Rc::new(RefCell::new(0_usize));
    let mut graph = Graph::<Local>::new();
    let factory_gate = gate.clone();
    let factory_count = Rc::clone(&factories);
    let node = graph
        .add_async("isolated", schema! { () -> () }, move |task, _inputs| {
            *factory_count.borrow_mut() += 1;
            let gate = factory_gate.clone();
            let cancellation = task.cancellation().cancelled();
            async move {
                GateOrCancellation {
                    gate: gate.wait(),
                    cancellation,
                }
                .await;
                Ok(outputs! {})
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let version = graph.compile().unwrap();

    let first = version.start(RunInputs::new()).unwrap();
    let first_control = first.control();
    let mut first = Box::pin(first);
    let mut second = Box::pin(version.start(RunInputs::new()).unwrap());
    let (_wake, waker) = counting_waker();
    let mut context = Context::from_waker(&waker);
    poll_until_pending(first.as_mut(), &mut context, || *factories.borrow() >= 1);
    poll_until_pending(second.as_mut(), &mut context, || *factories.borrow() >= 2);

    first_control.cancel();
    assert!(matches!(
        futures_lite::future::block_on(first),
        Err(ExecuteError::Cancelled(_))
    ));

    gate.release();
    futures_lite::future::block_on(second).unwrap();
    assert_eq!(*factories.borrow(), 2);
}

#[test]
fn dropping_a_pending_run_drops_its_future_once() {
    let gate = Gate::default();
    let mut graph = Graph::<Local>::new();
    let factory_gate = gate.clone();
    let node = graph
        .add_async("drop", schema! { () -> () }, move |_task, _inputs| {
            let gate = factory_gate.clone();
            async move {
                gate.wait().await;
                Ok(outputs! {})
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let mut run = Box::pin(graph.compile().unwrap().start(RunInputs::new()).unwrap());
    let (_wake, waker) = counting_waker();
    let mut context = Context::from_waker(&waker);
    poll_until_pending(run.as_mut(), &mut context, || gate.polls() > 0);

    drop(run);
    assert_eq!(gate.drops(), 1);
}

#[test]
fn runner_is_reusable_after_dropping_a_pending_run() {
    let gate = Gate::default();
    let factories = Rc::new(RefCell::new(0_usize));
    let mut graph = Graph::<Local>::new();
    let factory_gate = gate.clone();
    let factory_count = Rc::clone(&factories);
    let node = graph
        .add_async("frame", schema! { () -> () }, move |_task, _inputs| {
            let number = {
                let mut count = factory_count.borrow_mut();
                *count += 1;
                *count
            };
            let gate = factory_gate.clone();
            async move {
                if number == 1 {
                    gate.wait().await;
                }
                Ok(outputs! {})
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let version = graph.compile().unwrap();
    let mut runner = version.runner();

    let mut first = Box::pin(runner.start(RunInputs::new()).unwrap());
    let (_wake, waker) = counting_waker();
    let mut context = Context::from_waker(&waker);
    poll_until_pending(first.as_mut(), &mut context, || gate.polls() > 0);
    drop(first);
    assert_eq!(gate.drops(), 1);

    futures_lite::future::block_on(runner.execute(RunInputs::new())).unwrap();
    assert_eq!(*factories.borrow(), 2);
}

#[test]
fn cooperative_cancel_of_an_unresponsive_future_remains_pending() {
    let gate = Gate::default();
    let mut graph = Graph::<Local>::new();
    let factory_gate = gate.clone();
    let node = graph
        .add_async(
            "unresponsive",
            schema! { () -> () },
            move |_task, _inputs| {
                let gate = factory_gate.clone();
                async move {
                    gate.wait().await;
                    Ok(outputs! {})
                }
            },
        )
        .unwrap();
    graph.set_active(node, true).unwrap();
    let run = graph.compile().unwrap().start(RunInputs::new()).unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let (_wake, waker) = counting_waker();
    let mut context = Context::from_waker(&waker);
    poll_until_pending(run.as_mut(), &mut context, || gate.polls() > 0);

    control.cancel();
    for _ in 0..MAX_POLLS {
        assert!(matches!(run.as_mut().poll(&mut context), Poll::Pending));
    }
    assert!(gate.polls() > 0);
    drop(run);
}

#[test]
fn aborting_a_started_pending_future_drops_it_once_and_finishes() {
    let gate = Gate::default();
    let mut graph = Graph::<Local>::new();
    let factory_gate = gate.clone();
    let node = graph
        .add_async("abort", schema! { () -> () }, move |_task, _inputs| {
            let gate = factory_gate.clone();
            async move {
                gate.wait().await;
                Ok(outputs! {})
            }
        })
        .unwrap();
    graph.set_active(node, true).unwrap();
    let run = graph.compile().unwrap().start(RunInputs::new()).unwrap();
    let control = run.control();
    let mut run = Box::pin(run);
    let (_wake, waker) = counting_waker();
    let mut context = Context::from_waker(&waker);
    poll_until_pending(run.as_mut(), &mut context, || gate.polls() > 0);

    control.abort();
    assert!(matches!(
        futures_lite::future::block_on(run),
        Err(ExecuteError::Cancelled(_))
    ));
    assert_eq!(gate.drops(), 1);
}
