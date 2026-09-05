//! Per-run inputs, Future driving, optional Ready-node dispatch, cancellation,
//! and reusable runners. All runtime operations remain unimplemented.
//!
//! ```compile_fail
//! use slot_graph::{Graph, Local, RunInputs};
//! fn assert_send<T: Send>() {}
//! let version = Graph::<Local>::new().compile().unwrap();
//! let control = version.start(RunInputs::new()).unwrap().control();
//! assert_send::<slot_graph::RunControl<Local>>();
//! let _ = control;
//! ```
//!
//! ```compile_fail
//! use slot_graph::{Graph, Local, RunInputs};
//! let mut runner = Graph::<Local>::new().compile().unwrap().runner();
//! let first = runner.start(RunInputs::new()).unwrap();
//! let second = runner.start(RunInputs::new()).unwrap();
//! let _ = (first, second);
//! ```
//!
//! A Local node job cannot enter a cross-thread worker pool:
//! ```compile_fail
//! use slot_graph::{Local, NodeJob};
//! fn assert_send<T: Send>() {}
//! assert_send::<NodeJob<Local>>();
//! ```

use crate::{
    error::{DispatchError, ExecuteError, NodeError, StartError},
    handles::{InputSlot, NodeId},
    mode::{Mode, ValueFor},
    report::RunReport,
};
use std::{
    cell::Cell,
    future::Future,
    marker::{PhantomData, PhantomPinned},
    pin::Pin,
    task::{Context, Poll},
};

/// One Ready node invocation that an external dispatcher may schedule.
///
/// The job owns the immutable input snapshot and invokes or polls exactly one
/// node task. For SendMode it is Send and may enter a work-stealing pool; for
/// Local it is !Send and must remain on its owner thread. It is deliberately
/// !Sync and !Unpin: ownership transfers to one executor task, which must never
/// poll it concurrently.
///
/// Completion, validated outputs, panic, or premature job drop is reported to
/// the originating GraphRun through private shared state. The job never commits
/// outputs or unlocks successors itself. Dropping an accepted job is observable
/// as a Dispatch node failure unless the run has already cancelled that
/// invocation. The runtime storage remains unimplemented in this revision.
#[must_use = "a node job must be scheduled or explicitly rejected"]
pub struct NodeJob<M: Mode> {
    node: NodeId,
    _mode: PhantomData<M>,
    _not_sync: PhantomData<Cell<()>>,
    _pinned: PhantomPinned,
}

impl<M: Mode> NodeJob<M> {
    /// Returns the declaration node identity, for diagnostics or priority mapping.
    pub fn node_id(&self) -> NodeId {
        self.node
    }
}

impl<M: Mode> Future for NodeJob<M> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
    }
}

/// Executor-neutral boundary for scheduling individual Ready nodes.
///
/// Returning Ok transfers responsibility for polling the job to completion or
/// dropping it. A dropped accepted job reports a Dispatch failure back to its
/// run. Returning Err means this Ready node was rejected; GraphRun records the
/// same failure category and continues independent branches. Implementations
/// should enqueue promptly rather than block the GraphRun poller. An accepted
/// job must not be leaked: the core cannot detect a host that neither polls nor
/// drops it, and the associated run may remain pending indefinitely.
///
/// The trait is intentionally unaware of threads, priorities, pools, and I/O.
/// Send execution entry points additionally require the dispatcher to be Send
/// and Sync; Local entry points permit owned, thread-affine implementations.
pub trait NodeDispatcher<M: Mode>: 'static {
    /// Accepts ownership of one independently runnable node job.
    fn dispatch(&self, job: NodeJob<M>) -> Result<(), DispatchError>;
}

impl<M: Mode, F> NodeDispatcher<M> for F
where
    F: Fn(NodeJob<M>) -> Result<(), DispatchError> + 'static,
{
    fn dispatch(&self, job: NodeJob<M>) -> Result<(), DispatchError> {
        self(job)
    }
}

/// Cooperative cancellation state copied into every task context.
pub struct CancellationToken<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> Clone for CancellationToken<M> {
    fn clone(&self) -> Self {
        Self { _mode: PhantomData }
    }
}
impl<M: Mode> CancellationToken<M> {
    /// Returns whether cancellation has been requested; currently a stub.
    pub fn is_cancelled(&self) -> bool {
        unimplemented!()
    }
    /// Fails when cancelled; currently a stub.
    pub fn checkpoint(&self) -> Result<(), NodeError<M>> {
        unimplemented!()
    }
    /// Returns a future resolved by cancellation; currently a stub.
    pub fn cancelled(&self) -> Cancelled<M> {
        unimplemented!()
    }
}
/// Future returned by [`CancellationToken::cancelled`].
pub struct Cancelled<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> Future for Cancelled<M> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        unimplemented!()
    }
}
/// Typed identity of an input exposed at run start.
/// A key is reusable across runs; each compiled version validates its own binding.
pub struct RunInput<T: ?Sized, M: Mode> {
    _input: InputSlot<T>,
    _binding_generation: u64,
    _type: PhantomData<fn() -> T>,
    _mode: PhantomData<M>,
}
impl<T: ?Sized, M: Mode> Copy for RunInput<T, M> {}
impl<T: ?Sized, M: Mode> Clone for RunInput<T, M> {
    fn clone(&self) -> Self {
        *self
    }
}
/// Values supplied for exposed inputs of one run.
pub struct RunInputs<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> RunInputs<M> {
    /// Creates an empty input bag; currently a stub.
    pub fn new() -> Self {
        unimplemented!()
    }
}
impl<M: Mode> RunInputs<M> {
    /// Supplies one owned external value satisfying this mode. Currently a stub.
    pub fn insert<T: ValueFor<M>>(
        &mut self,
        _input: RunInput<T, M>,
        _value: T,
    ) -> Result<(), StartError> {
        unimplemented!()
    }
    /// Supplies an ordered collection for a Many input. Currently a stub.
    pub fn extend<T: ValueFor<M>, I: IntoIterator<Item = T>>(
        &mut self,
        _input: RunInput<T, M>,
        _values: I,
    ) -> Result<(), StartError> {
        unimplemented!()
    }
}
impl<M: Mode> Default for RunInputs<M> {
    fn default() -> Self {
        Self::new()
    }
}
/// An owned future that drives one graph execution.
///
/// Default runs poll Ready nodes inline. Runs created with an external
/// dispatcher remain the sole DAG orchestrator: they submit jobs, consume
/// completion notifications, atomically commit outputs, and unlock successors.
pub struct GraphRun<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> GraphRun<M> {
    /// Returns a cloneable external cancellation control; currently a stub.
    pub fn control(&self) -> RunControl<M> {
        unimplemented!()
    }
}
impl<M: Mode> Future for GraphRun<M> {
    type Output = Result<RunReport<M>, ExecuteError<M>>;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
    }
}
/// External cancellation and abort control for one run.
pub struct RunControl<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> Clone for RunControl<M> {
    fn clone(&self) -> Self {
        Self { _mode: PhantomData }
    }
}
impl<M: Mode> RunControl<M> {
    /// Requests cooperative cancellation; currently a stub.
    pub fn cancel(&self) {
        unimplemented!()
    }
    /// Requests pending futures be dropped on the next poll; currently a stub.
    pub fn abort(&self) {
        unimplemented!()
    }
}

/// Exclusive reusable storage for sequential graph runs.
///
/// A dispatcher-backed runner may allocate a fresh per-run generation when a
/// pending run is dropped; late jobs can therefore never write into storage
/// already reused by a later run.
pub struct GraphRunner<M: Mode> {
    _mode: PhantomData<M>,
}
impl<M: Mode> GraphRunner<M> {
    /// Borrows this runner until the returned run is dropped; currently a stub.
    pub fn start<'a>(&'a mut self, _inputs: RunInputs<M>) -> Result<RunnerRun<'a, M>, StartError> {
        unimplemented!()
    }
    /// Starts a borrowed runner future; currently a stub.
    pub fn execute<'a>(&'a mut self, _inputs: RunInputs<M>) -> RunnerRun<'a, M> {
        unimplemented!()
    }
    /// Releases retained capacity; currently a stub.
    pub fn trim(&mut self) {
        unimplemented!()
    }
}
/// Future borrowing a [`GraphRunner`] exclusively.
pub struct RunnerRun<'a, M: Mode> {
    _runner: PhantomData<&'a mut GraphRunner<M>>,
    _mode: PhantomData<M>,
}
impl<'a, M: Mode> RunnerRun<'a, M> {
    /// Returns run control; currently a stub.
    pub fn control(&self) -> RunControl<M> {
        unimplemented!()
    }
}
impl<'a, M: Mode> Future for RunnerRun<'a, M> {
    type Output = Result<RunReport<M>, ExecuteError<M>>;
    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        unimplemented!()
    }
}
