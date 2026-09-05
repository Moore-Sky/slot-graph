//! Per-run inputs, Future driving, optional Ready-node dispatch, cancellation,
//! and reusable runners.
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
    compiled::CompiledPlan,
    error::{
        DispatchError, ErrorContext, ExecuteError, NodeError, NodeErrorKind, StartError,
        StartErrorKind,
    },
    handles::{InputSlot, NodeId, RunId, SlotId},
    mode::{Mode, SendMode, ValueFor},
    report::{NodeFailure, NodeStatus, ReportNode, RunReport, TargetOutput},
    schema::Cardinality,
    task::{Task, TaskContext, TaskFuture, TaskInvocation, TaskResult},
    value::{NodeInputs, NodeOutputs, OutputAddress, StoredValue},
};
use std::{
    cell::Cell,
    collections::VecDeque,
    future::Future,
    marker::{PhantomData, PhantomPinned},
    panic::{catch_unwind, AssertUnwindSafe},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll},
};

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

struct CancelState {
    cancelled: AtomicBool,
    aborted: AtomicBool,
    start_gate: Mutex<()>,
    waiters: Mutex<Vec<std::task::Waker>>,
    driver: Mutex<Option<std::task::Waker>>,
}

impl CancelState {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            aborted: AtomicBool::new(false),
            start_gate: Mutex::new(()),
            waiters: Mutex::new(Vec::new()),
            driver: Mutex::new(None),
        }
    }

    fn wake_all(&self) {
        // A Waker is permitted to synchronously re-enter its executor. Never
        // invoke it while one of this run's coordination locks is held.
        let driver = self.driver.lock().unwrap().take();
        let waiters = {
            let mut waiters = self.waiters.lock().unwrap();
            std::mem::take(&mut *waiters)
        };
        if let Some(waker) = driver {
            waker.wake();
        }
        for waker in waiters {
            waker.wake();
        }
    }

    /// Registers an executor waker and reports whether cancellation already
    /// won the race with that registration. Cancellation first sets its atomic
    /// flag, then drains this same mutex, so the final check while holding the
    /// mutex closes the check-register-return-Pending window.
    ///
    /// Registration is retained even after cooperative cancellation. A later
    /// abort must still be able to wake a task that ignored the first request.
    fn register_waiter(&self, waker: &std::task::Waker) -> bool {
        // Cloning a RawWaker may execute foreign code. Keep that operation out
        // of the waiter mutex for the same re-entrancy reason as wake_all().
        let waker = waker.clone();
        let mut waiters = self.waiters.lock().unwrap();
        if !waiters
            .iter()
            .any(|registered| registered.will_wake(&waker))
        {
            waiters.push(waker);
        }
        self.cancelled.load(Ordering::Acquire) || self.aborted.load(Ordering::Acquire)
    }
}

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
/// invocation.
#[must_use = "a node job must be scheduled or explicitly rejected"]
pub struct NodeJob<M: Mode> {
    node: NodeId,
    index: usize,
    task: Option<Task<M>>,
    inputs: Option<NodeInputs<M>>,
    future: Option<TaskFuture<M>>,
    queue: Arc<JobQueue<M>>,
    cancel: Arc<CancelState>,
    completed: bool,
    cancel_catch_up_scheduled: bool,
    _mode: PhantomData<M>,
    _not_sync: PhantomData<Cell<()>>,
    _pinned: PhantomPinned,
}

impl<M: Mode> NodeJob<M> {
    fn new(
        node: NodeId,
        index: usize,
        task: Task<M>,
        inputs: NodeInputs<M>,
        queue: Arc<JobQueue<M>>,
        cancel: Arc<CancelState>,
    ) -> Self {
        Self {
            node,
            index,
            task: Some(task),
            inputs: Some(inputs),
            future: None,
            queue,
            cancel,
            completed: false,
            cancel_catch_up_scheduled: false,
            _mode: PhantomData,
            _not_sync: PhantomData,
            _pinned: PhantomPinned,
        }
    }

    /// Returns the declaration node identity, for diagnostics or priority mapping.
    pub fn node_id(&self) -> NodeId {
        self.node
    }
}

impl<M: Mode> Future for NodeJob<M> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // No field is structurally pinned; PhantomPinned only prevents callers
        // from moving the job after an executor has pinned it.
        let this = unsafe { self.get_unchecked_mut() };
        if this.completed {
            return Poll::Ready(());
        }
        if this.queue.retired.load(Ordering::Acquire) {
            this.future = None;
            this.task = None;
            this.inputs = None;
            this.completed = true;
            return Poll::Ready(());
        }
        if this.cancel.aborted.load(Ordering::Acquire) {
            this.future = None;
            this.task = None;
            this.inputs = None;
            this.completed = true;
            this.queue.push(JobEvent::Cancelled(this.index));
            return Poll::Ready(());
        }
        if this.future.is_none() {
            let (task, inputs) = {
                let _start = this.cancel.start_gate.lock().unwrap();
                if this.queue.retired.load(Ordering::Acquire)
                    || this.cancel.cancelled.load(Ordering::Acquire)
                {
                    this.task = None;
                    this.inputs = None;
                    this.completed = true;
                    this.queue.push(JobEvent::Cancelled(this.index));
                    return Poll::Ready(());
                }
                (
                    this.task.take().expect("node job factory exists"),
                    this.inputs.take().expect("node job inputs exist"),
                )
            };
            let token = CancellationToken {
                state: Arc::clone(&this.cancel),
                _mode: PhantomData,
            };
            match catch_unwind(AssertUnwindSafe(|| {
                task.invoke(TaskContext::new(this.node, token), inputs)
            })) {
                Ok(TaskInvocation::Sync(result)) => {
                    this.completed = true;
                    this.queue.push(JobEvent::Complete(this.index, result));
                    return Poll::Ready(());
                }
                Ok(TaskInvocation::Async(future)) => this.future = Some(future),
                Err(_) => {
                    this.completed = true;
                    this.queue.push(JobEvent::Panic(this.index));
                    return Poll::Ready(());
                }
            }
        }
        let future = this.future.as_mut().expect("node job future exists");
        match catch_unwind(AssertUnwindSafe(|| Pin::new(future).poll(cx))) {
            Ok(Poll::Pending) => {
                if this.cancel.register_waiter(cx.waker()) {
                    if this.queue.retired.load(Ordering::Acquire) {
                        this.future = None;
                        this.task = None;
                        this.inputs = None;
                        this.completed = true;
                        return Poll::Ready(());
                    }
                    if this.cancel.aborted.load(Ordering::Acquire) {
                        this.future = None;
                        this.task = None;
                        this.inputs = None;
                        this.completed = true;
                        this.queue.push(JobEvent::Cancelled(this.index));
                        return Poll::Ready(());
                    }

                    // Cooperative cancellation does not drop an unresponsive
                    // task. If cancellation happened after its child Future
                    // returned Pending but before a usable waiter was present,
                    // no earlier wake could have reached this NodeJob. Schedule
                    // one catch-up poll so a cancellation-aware child can
                    // observe its token without turning an ignoring child into
                    // a self-waking busy loop.
                    if !this.cancel_catch_up_scheduled {
                        this.cancel_catch_up_scheduled = true;
                        cx.waker().wake_by_ref();
                    }
                }
                Poll::Pending
            }
            Ok(Poll::Ready(result)) => {
                this.future = None;
                this.completed = true;
                this.queue.push(JobEvent::Complete(this.index, result));
                Poll::Ready(())
            }
            Err(_) => {
                this.future = None;
                this.completed = true;
                this.queue.push(JobEvent::Panic(this.index));
                Poll::Ready(())
            }
        }
    }
}

impl<M: Mode> Drop for NodeJob<M> {
    fn drop(&mut self) {
        if !self.completed && !self.queue.retired.load(Ordering::Acquire) {
            self.completed = true;
            self.queue.push(JobEvent::Dropped(self.index));
        }
    }
}

enum JobEvent<M: Mode> {
    Complete(usize, TaskResult<M>),
    Panic(usize),
    Dropped(usize),
    Cancelled(usize),
}

struct JobQueue<M: Mode> {
    events: Mutex<VecDeque<JobEvent<M>>>,
    driver: Mutex<Option<std::task::Waker>>,
    retired: AtomicBool,
}

impl<M: Mode> JobQueue<M> {
    fn new() -> Self {
        Self {
            events: Mutex::new(VecDeque::new()),
            driver: Mutex::new(None),
            retired: AtomicBool::new(false),
        }
    }

    fn push(&self, event: JobEvent<M>) {
        self.events.lock().unwrap().push_back(event);
        // See CancelState::wake_all: releasing the driver lock before wake is
        // required for synchronous executor implementations.
        let driver = self.driver.lock().unwrap().take();
        if let Some(waker) = driver {
            waker.wake();
        }
    }
}

pub(crate) struct Dispatcher<M: Mode> {
    inner: Arc<dyn NodeDispatcher<M>>,
}

impl<M: Mode> Clone for Dispatcher<M> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<M: Mode> Dispatcher<M> {
    pub(crate) fn new<D: NodeDispatcher<M>>(dispatcher: D) -> Self {
        Self {
            inner: Arc::new(dispatcher),
        }
    }
}

trait SendDispatcherMode: Mode + Send + Sync {}
impl SendDispatcherMode for SendMode {}

// SendMode constructors accept only Send + Sync dispatchers. Type erasure
// removes those auto-traits from the object type, so this private marker
// restores exactly the invariant established at construction.
unsafe impl<M: SendDispatcherMode> Send for Dispatcher<M> {}
unsafe impl<M: SendDispatcherMode> Sync for Dispatcher<M> {}

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
    state: Arc<CancelState>,
    _mode: PhantomData<M>,
}
impl<M: Mode> Clone for CancellationToken<M> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            _mode: PhantomData,
        }
    }
}
impl<M: Mode> CancellationToken<M> {
    /// Returns whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
    }
    /// Fails when cancelled.
    pub fn checkpoint(&self) -> Result<(), NodeError<M>> {
        if self.is_cancelled() {
            Err(NodeError::internal(
                NodeErrorKind::Cancelled,
                ErrorContext::default(),
            ))
        } else {
            Ok(())
        }
    }
    /// Returns a future resolved by cancellation.
    pub fn cancelled(&self) -> Cancelled<M> {
        Cancelled {
            state: Arc::clone(&self.state),
            _mode: PhantomData,
        }
    }
}
/// Future returned by [`CancellationToken::cancelled`].
pub struct Cancelled<M: Mode> {
    state: Arc<CancelState>,
    _mode: PhantomData<M>,
}
impl<M: Mode> Future for Cancelled<M> {
    type Output = ();
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.state.cancelled.load(Ordering::Acquire) {
            return Poll::Ready(());
        }
        if self.state.register_waiter(cx.waker()) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
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
impl<T: ?Sized, M: Mode> RunInput<T, M> {
    pub(crate) fn new(input: InputSlot<T>, binding_generation: u64) -> Self {
        Self {
            _input: input,
            _binding_generation: binding_generation,
            _type: PhantomData,
            _mode: PhantomData,
        }
    }

    pub(crate) fn parts(self) -> (InputSlot<T>, u64) {
        (self._input, self._binding_generation)
    }
}
/// Values supplied for exposed inputs of one run.
pub struct RunInputs<M: Mode> {
    entries: Vec<RunInputEntry>,
    _mode: PhantomData<M>,
}
struct RunInputEntry {
    node: NodeId,
    slot: SlotId,
    binding: u64,
    shape: SuppliedShape,
    values: Vec<StoredValue>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum SuppliedShape {
    One,
    Many,
}
impl<M: Mode> RunInputs<M> {
    /// Creates an empty input bag.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            _mode: PhantomData,
        }
    }
}
impl<M: Mode> RunInputs<M> {
    /// Supplies one owned external value satisfying this mode.
    pub fn insert<T: ValueFor<M>>(
        &mut self,
        input: RunInput<T, M>,
        value: T,
    ) -> Result<(), StartError> {
        let (input, binding) = input.parts();
        let (node, slot, _) = input.parts();
        self.push_entry(RunInputEntry {
            node,
            slot,
            binding,
            shape: SuppliedShape::One,
            values: vec![StoredValue::from_value::<T, M>(value)],
        })
    }
    /// Supplies an ordered collection for a Many input.
    pub fn extend<T: ValueFor<M>, I: IntoIterator<Item = T>>(
        &mut self,
        input: RunInput<T, M>,
        values: I,
    ) -> Result<(), StartError> {
        let (input, binding) = input.parts();
        let (node, slot, _) = input.parts();
        self.push_entry(RunInputEntry {
            node,
            slot,
            binding,
            shape: SuppliedShape::Many,
            values: values
                .into_iter()
                .map(StoredValue::from_value::<T, M>)
                .collect(),
        })
    }

    fn push_entry(&mut self, entry: RunInputEntry) -> Result<(), StartError> {
        if self
            .entries
            .iter()
            .any(|existing| existing.binding == entry.binding)
        {
            return Err(start_error(
                StartErrorKind::DuplicateRunInput,
                Some(entry.node),
            ));
        }
        self.entries.push(entry);
        Ok(())
    }
}
impl<M: Mode> Default for RunInputs<M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Reusable, exclusively-owned storage for one graph run.
///
/// This deliberately excludes cancellation and dispatch coordination. Both can
/// be retained by task contexts or jobs after their originating run is dropped,
/// so they must always have a fresh allocation for the next run generation.
struct RunScratch<M: Mode> {
    statuses: Vec<NodeStatus>,
    remaining: Vec<usize>,
    outputs: Vec<Vec<Option<StoredValue>>>,
    external: Vec<Vec<Vec<StoredValue>>>,
    futures: Vec<Option<TaskFuture<M>>>,
    ready: VecDeque<usize>,
    failures: Vec<Option<NodeError<M>>>,
    blocked_by: Vec<Option<NodeId>>,
}

impl<M: Mode> RunScratch<M> {
    fn new(plan: &CompiledPlan<M>) -> Self {
        let mut scratch = Self {
            statuses: Vec::with_capacity(plan.nodes.len()),
            remaining: Vec::with_capacity(plan.nodes.len()),
            outputs: Vec::with_capacity(plan.nodes.len()),
            external: Vec::with_capacity(plan.nodes.len()),
            futures: Vec::with_capacity(plan.nodes.len()),
            ready: VecDeque::new(),
            failures: Vec::with_capacity(plan.nodes.len()),
            blocked_by: Vec::with_capacity(plan.nodes.len()),
        };
        scratch.reset(plan);
        scratch
    }

    /// Drops values owned by the completed or abandoned generation while
    /// retaining the containers and their capacity for the runner's next run.
    fn reset(&mut self, plan: &CompiledPlan<M>) {
        let node_count = plan.nodes.len();

        self.statuses.clear();
        self.statuses.resize(node_count, NodeStatus::Pending);

        self.remaining.clear();
        self.remaining
            .extend(plan.nodes.iter().map(|node| node.predecessors.len()));

        self.outputs.resize_with(node_count, Vec::new);
        self.external.resize_with(node_count, Vec::new);
        for (index, node) in plan.nodes.iter().enumerate() {
            let outputs = &mut self.outputs[index];
            outputs.clear();
            outputs.resize_with(node.schema.schema().outputs.len(), || None);

            let external = &mut self.external[index];
            external.resize_with(node.inputs.len(), Vec::new);
            for values in external.iter_mut() {
                values.clear();
            }
        }

        self.futures.clear();
        self.futures.resize_with(node_count, || None);
        self.ready.clear();

        self.failures.clear();
        self.failures.resize_with(node_count, || None);

        self.blocked_by.clear();
        self.blocked_by.resize(node_count, None);
    }
}

/// An owned future that drives one graph execution.
///
/// Default runs poll Ready nodes inline. Runs created with an external
/// dispatcher remain the sole DAG orchestrator: they submit jobs, consume
/// completion notifications, atomically commit outputs, and unlock successors.
pub struct GraphRun<M: Mode> {
    plan: Arc<CompiledPlan<M>>,
    run_id: RunId,
    cancel: Arc<CancelState>,
    statuses: Vec<NodeStatus>,
    remaining: Vec<usize>,
    outputs: Vec<Vec<Option<StoredValue>>>,
    external: Vec<Vec<Vec<StoredValue>>>,
    futures: Vec<Option<TaskFuture<M>>>,
    ready: VecDeque<usize>,
    failures: Vec<Option<NodeError<M>>>,
    blocked_by: Vec<Option<NodeId>>,
    dispatcher: Option<Dispatcher<M>>,
    job_queue: Arc<JobQueue<M>>,
    initialized: bool,
    start_error: Option<StartError>,
    finished: bool,
    _mode: PhantomData<M>,
}
impl<M: Mode> GraphRun<M> {
    pub(crate) fn start_inline(
        plan: Arc<CompiledPlan<M>>,
        inputs: RunInputs<M>,
    ) -> Result<Self, StartError> {
        let mut scratch = RunScratch::new(&plan);
        validate_run_inputs_into(&plan, inputs, &mut scratch.external)?;
        Ok(Self::new(plan, scratch, None, None))
    }

    pub(crate) fn execute_inline(plan: Arc<CompiledPlan<M>>, inputs: RunInputs<M>) -> Self {
        let mut scratch = RunScratch::new(&plan);
        let start_error = validate_run_inputs_into(&plan, inputs, &mut scratch.external).err();
        Self::new(plan, scratch, start_error, None)
    }

    fn new(
        plan: Arc<CompiledPlan<M>>,
        scratch: RunScratch<M>,
        start_error: Option<StartError>,
        dispatcher: Option<Dispatcher<M>>,
    ) -> Self {
        Self {
            run_id: RunId(NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed)),
            cancel: Arc::new(CancelState::new()),
            statuses: scratch.statuses,
            remaining: scratch.remaining,
            outputs: scratch.outputs,
            external: scratch.external,
            futures: scratch.futures,
            ready: scratch.ready,
            failures: scratch.failures,
            blocked_by: scratch.blocked_by,
            dispatcher,
            job_queue: Arc::new(JobQueue::new()),
            initialized: false,
            start_error,
            finished: false,
            plan,
            _mode: PhantomData,
        }
    }

    pub(crate) fn start_dispatched(
        plan: Arc<CompiledPlan<M>>,
        inputs: RunInputs<M>,
        dispatcher: Dispatcher<M>,
    ) -> Result<Self, StartError> {
        let mut scratch = RunScratch::new(&plan);
        validate_run_inputs_into(&plan, inputs, &mut scratch.external)?;
        Ok(Self::new(plan, scratch, None, Some(dispatcher)))
    }

    pub(crate) fn execute_dispatched(
        plan: Arc<CompiledPlan<M>>,
        inputs: RunInputs<M>,
        dispatcher: Dispatcher<M>,
    ) -> Self {
        let mut scratch = RunScratch::new(&plan);
        let start_error = validate_run_inputs_into(&plan, inputs, &mut scratch.external).err();
        Self::new(plan, scratch, start_error, Some(dispatcher))
    }

    /// Returns a cloneable external cancellation control.
    pub fn control(&self) -> RunControl<M> {
        RunControl {
            state: Arc::clone(&self.cancel),
            _mode: PhantomData,
        }
    }

    fn retire(&self) {
        let first_retirement = {
            let _start = self.cancel.start_gate.lock().unwrap();
            if self.job_queue.retired.swap(true, Ordering::AcqRel) {
                false
            } else {
                self.cancel.cancelled.store(true, Ordering::Release);
                self.cancel.aborted.store(true, Ordering::Release);
                true
            }
        };
        if first_retirement {
            self.cancel.wake_all();
        }
    }

    fn take_scratch_for_reuse(&mut self) -> RunScratch<M> {
        self.retire();
        let mut scratch = RunScratch {
            statuses: std::mem::take(&mut self.statuses),
            remaining: std::mem::take(&mut self.remaining),
            outputs: std::mem::take(&mut self.outputs),
            external: std::mem::take(&mut self.external),
            futures: std::mem::take(&mut self.futures),
            ready: std::mem::take(&mut self.ready),
            failures: std::mem::take(&mut self.failures),
            blocked_by: std::mem::take(&mut self.blocked_by),
        };
        scratch.reset(&self.plan);
        scratch
    }
}
impl<M: Mode> Future for GraphRun<M> {
    type Output = Result<RunReport<M>, ExecuteError<M>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        assert!(!this.finished, "GraphRun polled after completion");
        if let Some(error) = this.start_error.take() {
            this.finished = true;
            return Poll::Ready(Err(ExecuteError::Start(error)));
        }
        *this.cancel.driver.lock().unwrap() = Some(cx.waker().clone());
        *this.job_queue.driver.lock().unwrap() = Some(cx.waker().clone());
        if !this.initialized {
            this.initialized = true;
            if this.cancel.cancelled.load(Ordering::Acquire) {
                this.cancel_unstarted();
            } else {
                for index in 0..this.plan.nodes.len() {
                    if this.remaining[index] == 0 {
                        this.statuses[index] = NodeStatus::Ready;
                        this.ready.push_back(index);
                    }
                }
            }
        }

        loop {
            let mut progressed = false;
            loop {
                let event = { this.job_queue.events.lock().unwrap().pop_front() };
                let Some(event) = event else {
                    break;
                };
                progressed = true;
                let index = match &event {
                    JobEvent::Complete(index, _)
                    | JobEvent::Panic(index)
                    | JobEvent::Dropped(index)
                    | JobEvent::Cancelled(index) => *index,
                };
                if this.statuses[index] != NodeStatus::Running {
                    continue;
                }
                match event {
                    JobEvent::Complete(_, result) => match result {
                        Ok(outputs) => match this.validate_outputs(index, outputs) {
                            Ok(values) => {
                                if this.cancel.cancelled.load(Ordering::Acquire) {
                                    this.statuses[index] = NodeStatus::Cancelled;
                                } else {
                                    this.commit_node(index, values);
                                }
                            }
                            Err(error) => this.fail_node(index, error),
                        },
                        Err(mut error) => {
                            if error.context.node.is_none() {
                                error.context.node = Some(this.plan.nodes[index].id);
                            }
                            if error.kind == NodeErrorKind::Cancelled
                                && this.cancel.cancelled.load(Ordering::Acquire)
                            {
                                this.statuses[index] = NodeStatus::Cancelled;
                            } else {
                                this.fail_node(index, error);
                            }
                        }
                    },
                    JobEvent::Panic(_) => {
                        let error = this.node_error(index, NodeErrorKind::Panic);
                        this.fail_node(index, error);
                    }
                    JobEvent::Dropped(_) => {
                        if this.cancel.cancelled.load(Ordering::Acquire) {
                            this.statuses[index] = NodeStatus::Cancelled;
                        } else {
                            let error = this.node_error(index, NodeErrorKind::Dispatch);
                            this.fail_node(index, error);
                        }
                    }
                    JobEvent::Cancelled(_) => this.statuses[index] = NodeStatus::Cancelled,
                }
            }
            if this.cancel.aborted.load(Ordering::Acquire) {
                for (index, future) in this.futures.iter_mut().enumerate() {
                    if future.take().is_some() {
                        progressed = true;
                        this.statuses[index] = NodeStatus::Cancelled;
                    }
                }
                this.cancel_unstarted();
            } else if this.cancel.cancelled.load(Ordering::Acquire) {
                this.cancel_unstarted();
            }

            while !this.cancel.cancelled.load(Ordering::Acquire) {
                let Some(index) = this.ready.pop_front() else {
                    break;
                };
                if this.statuses[index] != NodeStatus::Ready {
                    continue;
                }
                progressed = true;
                if let Some(dispatcher) = this.dispatcher.clone() {
                    match this.build_inputs(index) {
                        Ok(inputs) => {
                            this.statuses[index] = NodeStatus::Running;
                            let job = NodeJob::new(
                                this.plan.nodes[index].id,
                                index,
                                this.plan.nodes[index].task.clone(),
                                inputs,
                                Arc::clone(&this.job_queue),
                                Arc::clone(&this.cancel),
                            );
                            let dispatched =
                                catch_unwind(AssertUnwindSafe(|| dispatcher.inner.dispatch(job)));
                            if this.statuses[index] == NodeStatus::Running {
                                match dispatched {
                                    Ok(Ok(())) => {}
                                    Ok(Err(source)) => {
                                        let error = this.node_dispatch_error(index, source);
                                        this.fail_node(index, error);
                                    }
                                    Err(_) => {
                                        let error = this.node_error(index, NodeErrorKind::Dispatch);
                                        this.fail_node(index, error);
                                    }
                                }
                            }
                        }
                        Err(error) => this.fail_node(index, error),
                    }
                } else {
                    match this.build_inputs(index) {
                        Ok(inputs) => {
                            let task = this.plan.nodes[index].task.clone();
                            let node = this.plan.nodes[index].id;
                            let cancel = Arc::clone(&this.cancel);
                            let claimed = {
                                let _start = cancel.start_gate.lock().unwrap();
                                if cancel.cancelled.load(Ordering::Acquire) {
                                    false
                                } else {
                                    this.statuses[index] = NodeStatus::Running;
                                    true
                                }
                            };
                            if !claimed {
                                this.statuses[index] = NodeStatus::Cancelled;
                                continue;
                            }
                            let token = CancellationToken {
                                state: Arc::clone(&this.cancel),
                                _mode: PhantomData,
                            };
                            match catch_unwind(AssertUnwindSafe(|| {
                                task.invoke(TaskContext::new(node, token), inputs)
                            })) {
                                Ok(TaskInvocation::Sync(result)) => {
                                    this.complete_inline(index, result);
                                }
                                Ok(TaskInvocation::Async(future)) => {
                                    this.futures[index] = Some(future);
                                }
                                Err(_) => {
                                    let error = this.node_error(index, NodeErrorKind::Panic);
                                    this.fail_node(index, error)
                                }
                            }
                        }
                        Err(error) => this.fail_node(index, error),
                    }
                }
            }

            for index in 0..this.futures.len() {
                let Some(mut future) = this.futures[index].take() else {
                    continue;
                };
                let polled = catch_unwind(AssertUnwindSafe(|| Pin::new(&mut future).poll(cx)));
                match polled {
                    Ok(Poll::Pending) => this.futures[index] = Some(future),
                    Ok(Poll::Ready(result)) => {
                        progressed = true;
                        this.complete_inline(index, result);
                    }
                    Err(_) => {
                        progressed = true;
                        let error = this.node_error(index, NodeErrorKind::Panic);
                        this.fail_node(index, error);
                    }
                }
            }

            if this.is_terminal() {
                this.finished = true;
                let cancelled = this.cancel.cancelled.load(Ordering::Acquire);
                let failed = this.failures.iter().any(Option::is_some);
                let report = this.take_report();
                return Poll::Ready(if cancelled {
                    Err(ExecuteError::Cancelled(report))
                } else if failed {
                    Err(ExecuteError::Failed(report))
                } else {
                    Ok(report)
                });
            }
            if !progressed {
                return Poll::Pending;
            }
        }
    }
}

impl<M: Mode> Unpin for GraphRun<M> {}

impl<M: Mode> Drop for GraphRun<M> {
    fn drop(&mut self) {
        self.retire();
    }
}

impl<M: Mode> GraphRun<M> {
    /// Applies the result of an inline task after it has completed. Synchronous
    /// tasks reach this directly; asynchronous tasks reach it after polling.
    /// Both routes therefore share the same validation, cancellation, and
    /// atomic-commit boundary.
    fn complete_inline(&mut self, index: usize, result: TaskResult<M>) {
        match result {
            Ok(outputs) => match self.validate_outputs(index, outputs) {
                Ok(values) => {
                    if self.cancel.cancelled.load(Ordering::Acquire) {
                        self.statuses[index] = NodeStatus::Cancelled;
                    } else {
                        self.commit_node(index, values);
                    }
                }
                Err(error) => self.fail_node(index, error),
            },
            Err(mut error) => {
                if error.context.node.is_none() {
                    error.context.node = Some(self.plan.nodes[index].id);
                }
                if error.kind == NodeErrorKind::Cancelled
                    && self.cancel.cancelled.load(Ordering::Acquire)
                {
                    self.statuses[index] = NodeStatus::Cancelled;
                } else {
                    self.fail_node(index, error);
                }
            }
        }
    }

    fn build_inputs(&self, index: usize) -> Result<NodeInputs<M>, NodeError<M>> {
        let node = &self.plan.nodes[index];
        let value_capacity =
            node.inputs
                .iter()
                .enumerate()
                .fold(0, |total, (input_index, input)| {
                    total
                        + if input.external.is_some() {
                            self.external[index][input_index].len()
                        } else {
                            input.sources.len()
                        }
                });
        let mut values = Vec::with_capacity(value_capacity);
        let mut ranges = Vec::with_capacity(node.inputs.len());
        for (input_index, input) in node.inputs.iter().enumerate() {
            let start = values.len();
            if input.external.is_some() {
                values.extend(self.external[index][input_index].iter().cloned());
                ranges.push(start..values.len());
                continue;
            }
            for source in &input.sources {
                let value = self.outputs[source.node][source.output]
                    .as_ref()
                    .ok_or_else(|| {
                        self.node_error(index, NodeErrorKind::InternalInvariantViolation)
                    })?
                    .clone();
                values.push(value);
            }
            ranges.push(start..values.len());
        }
        Ok(NodeInputs::from_resolved(
            Arc::clone(&node.input_layout),
            ranges,
            values,
        ))
    }

    fn validate_outputs(
        &self,
        index: usize,
        outputs: NodeOutputs<M>,
    ) -> Result<Vec<StoredValue>, NodeError<M>> {
        let node = &self.plan.nodes[index];
        let specs = &node.schema.schema().outputs;
        let mut resolved: Vec<Option<StoredValue>> =
            std::iter::repeat_with(|| None).take(specs.len()).collect();
        for entry in outputs.into_entries() {
            let output_index = match entry.address {
                OutputAddress::Name(name) => specs.iter().position(|spec| spec.name == name),
                OutputAddress::Key {
                    layout,
                    index: output,
                } if layout == node.schema.layout() && output < specs.len() => Some(output),
                OutputAddress::Key { .. } => None,
            }
            .ok_or_else(|| self.node_error(index, NodeErrorKind::InvalidOutputs))?;
            if resolved[output_index].is_some()
                || !specs[output_index]
                    .value_type
                    .matches(entry.value.type_id())
            {
                return Err(self.node_error(index, NodeErrorKind::InvalidOutputs));
            }
            resolved[output_index] = Some(entry.value);
        }
        if resolved.iter().any(Option::is_none) {
            return Err(self.node_error(index, NodeErrorKind::InvalidOutputs));
        }
        Ok(resolved.into_iter().map(Option::unwrap).collect())
    }

    fn commit_node(&mut self, index: usize, values: Vec<StoredValue>) {
        let slots = &mut self.outputs[index];
        debug_assert_eq!(slots.len(), values.len());
        for (slot, value) in slots.iter_mut().zip(values) {
            *slot = Some(value);
        }
        self.statuses[index] = NodeStatus::Succeeded;
        let successors = self.plan.nodes[index].successors.clone();
        for successor in successors {
            if self.statuses[successor] != NodeStatus::Pending {
                continue;
            }
            self.remaining[successor] = self.remaining[successor].saturating_sub(1);
            if self.remaining[successor] == 0 {
                self.statuses[successor] = NodeStatus::Ready;
                self.ready.push_back(successor);
            }
        }
    }

    fn fail_node(&mut self, index: usize, error: NodeError<M>) {
        self.statuses[index] = NodeStatus::Failed;
        self.failures[index] = Some(error);
        let failed = self.plan.nodes[index].id;
        let successors = self.plan.nodes[index].successors.clone();
        for successor in successors {
            self.block_branch(successor, failed);
        }
    }

    fn block_branch(&mut self, index: usize, predecessor: NodeId) {
        if matches!(
            self.statuses[index],
            NodeStatus::Succeeded
                | NodeStatus::Failed
                | NodeStatus::Cancelled
                | NodeStatus::Blocked
        ) {
            return;
        }
        self.futures[index] = None;
        self.statuses[index] = NodeStatus::Blocked;
        self.blocked_by[index] = Some(predecessor);
        let blocked = self.plan.nodes[index].id;
        let successors = self.plan.nodes[index].successors.clone();
        for successor in successors {
            self.block_branch(successor, blocked);
        }
    }

    fn cancel_unstarted(&mut self) {
        for status in &mut self.statuses {
            if matches!(*status, NodeStatus::Pending | NodeStatus::Ready) {
                *status = NodeStatus::Cancelled;
            }
        }
        self.ready.clear();
    }

    fn is_terminal(&self) -> bool {
        self.statuses.iter().all(|status| {
            matches!(
                status,
                NodeStatus::Succeeded
                    | NodeStatus::Failed
                    | NodeStatus::Cancelled
                    | NodeStatus::Blocked
            )
        })
    }

    fn node_error(&self, index: usize, kind: NodeErrorKind) -> NodeError<M> {
        NodeError::internal(
            kind,
            ErrorContext {
                graph: Some(self.plan.graph),
                node: Some(self.plan.nodes[index].id),
                name: Some(self.plan.nodes[index].name.clone()),
                ..ErrorContext::default()
            },
        )
    }

    fn node_dispatch_error(&self, index: usize, source: DispatchError) -> NodeError<M> {
        NodeError::dispatch(
            source,
            ErrorContext {
                graph: Some(self.plan.graph),
                node: Some(self.plan.nodes[index].id),
                name: Some(self.plan.nodes[index].name.clone()),
                ..ErrorContext::default()
            },
        )
    }

    fn take_report(&mut self) -> RunReport<M> {
        let nodes = self
            .plan
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| ReportNode {
                node: node.id,
                status: self.statuses[index],
                blocked_by: self.blocked_by[index],
                outputs: node
                    .schema
                    .schema()
                    .outputs
                    .iter()
                    .map(|output| (output.id, node.schema_generation))
                    .collect(),
            })
            .collect();
        let mut targets = Vec::new();
        for (node_index, node) in self.plan.nodes.iter().enumerate() {
            if !node.active {
                continue;
            }
            for (output_index, output) in node.schema.schema().outputs.iter().enumerate() {
                if let Some(value) = self.outputs[node_index][output_index].take() {
                    targets.push(TargetOutput::available(
                        node.id,
                        output.id,
                        node.schema_generation,
                        value,
                    ));
                } else {
                    targets.push(TargetOutput::unavailable(
                        node.id,
                        output.id,
                        node.schema_generation,
                    ));
                }
            }
        }
        let failures = self
            .failures
            .iter_mut()
            .enumerate()
            .filter_map(|(index, error)| {
                error.take().map(|error| NodeFailure {
                    node: self.plan.nodes[index].id,
                    error,
                })
            })
            .collect();
        RunReport::new(
            self.plan.graph,
            self.plan.version,
            self.run_id,
            nodes,
            failures,
            targets,
        )
    }
}

fn validate_run_inputs_into<M: Mode>(
    plan: &CompiledPlan<M>,
    inputs: RunInputs<M>,
    resolved: &mut [Vec<Vec<StoredValue>>],
) -> Result<(), StartError> {
    debug_assert_eq!(resolved.len(), plan.nodes.len());
    // Validate the complete bag before moving any values into reusable
    // storage. A failed start must not retain caller-provided resources until
    // the runner's next use.
    for supplied in &inputs.entries {
        let Some(&node_index) = plan.node_index.get(&supplied.node) else {
            return Err(start_error(
                StartErrorKind::UnexpectedRunInput,
                Some(supplied.node),
            ));
        };
        let node = &plan.nodes[node_index];
        let Some((_input_index, external)) = node
            .schema
            .schema()
            .inputs
            .iter()
            .enumerate()
            .find_map(|(input_index, spec)| {
                let external = node.inputs[input_index].external?;
                (spec.id == supplied.slot && external.binding == supplied.binding)
                    .then_some((input_index, external))
            })
        else {
            return Err(start_error(
                StartErrorKind::UnexpectedRunInput,
                Some(supplied.node),
            ));
        };
        let shape_matches = matches!(
            (supplied.shape, external.cardinality),
            (SuppliedShape::One, Cardinality::One) | (SuppliedShape::Many, Cardinality::Many)
        );
        if !shape_matches
            || (external.cardinality == Cardinality::One && supplied.values.len() != 1)
        {
            return Err(start_error(
                StartErrorKind::RunInputCardinality,
                Some(supplied.node),
            ));
        }
        if supplied
            .values
            .iter()
            .any(|value| !external.value_type.matches(value.type_id()))
        {
            return Err(start_error(
                StartErrorKind::RunInputTypeMismatch,
                Some(supplied.node),
            ));
        }
    }
    for node in &plan.nodes {
        for input in &node.inputs {
            let Some(external) = input.external else {
                continue;
            };
            if external.presence == crate::schema::Presence::Required
                && !inputs.entries.iter().any(|supplied| {
                    supplied.node == node.id
                        && supplied.binding == external.binding
                        && !supplied.values.is_empty()
                })
            {
                return Err(start_error(StartErrorKind::MissingRunInput, Some(node.id)));
            }
        }
    }

    for supplied in inputs.entries {
        let node_index = plan.node_index[&supplied.node];
        let node = &plan.nodes[node_index];
        let input_index = node
            .schema
            .schema()
            .inputs
            .iter()
            .enumerate()
            .find_map(|(input_index, spec)| {
                let external = node.inputs[input_index].external?;
                (spec.id == supplied.slot && external.binding == supplied.binding)
                    .then_some(input_index)
            })
            .expect("validated run input remains bound");
        resolved[node_index][input_index].extend(supplied.values);
    }
    Ok(())
}

fn start_error(kind: StartErrorKind, node: Option<NodeId>) -> StartError {
    StartError::new(
        kind,
        ErrorContext {
            node,
            graph: node.map(NodeId::graph),
            ..ErrorContext::default()
        },
    )
}
/// External cancellation and abort control for one run.
pub struct RunControl<M: Mode> {
    state: Arc<CancelState>,
    _mode: PhantomData<M>,
}
impl<M: Mode> Clone for RunControl<M> {
    fn clone(&self) -> Self {
        Self {
            state: Arc::clone(&self.state),
            _mode: PhantomData,
        }
    }
}
impl<M: Mode> RunControl<M> {
    /// Requests cooperative cancellation and prevents new task claims.
    pub fn cancel(&self) {
        {
            let _start = self.state.start_gate.lock().unwrap();
            self.state.cancelled.store(true, Ordering::Release);
        }
        self.state.wake_all();
    }
    /// Prevents new task claims and requests pending futures be dropped.
    pub fn abort(&self) {
        {
            let _start = self.state.start_gate.lock().unwrap();
            self.state.cancelled.store(true, Ordering::Release);
            self.state.aborted.store(true, Ordering::Release);
        }
        self.state.wake_all();
    }
}

/// Exclusive reusable storage for sequential graph runs.
///
/// A dispatcher-backed runner may allocate a fresh per-run generation when a
/// pending run is dropped; late jobs can therefore never write into storage
/// already reused by a later run.
pub struct GraphRunner<M: Mode> {
    plan: Arc<CompiledPlan<M>>,
    dispatcher: Option<Dispatcher<M>>,
    scratch: Option<RunScratch<M>>,
    _mode: PhantomData<M>,
}
impl<M: Mode> GraphRunner<M> {
    pub(crate) fn new(version: crate::compiled::ExecutionGraphVersion<M>) -> Self {
        Self {
            plan: version.plan,
            dispatcher: None,
            scratch: None,
            _mode: PhantomData,
        }
    }

    pub(crate) fn new_on(
        version: crate::compiled::ExecutionGraphVersion<M>,
        dispatcher: Dispatcher<M>,
    ) -> Self {
        Self {
            plan: version.plan,
            dispatcher: Some(dispatcher),
            scratch: None,
            _mode: PhantomData,
        }
    }

    fn take_scratch(&mut self) -> RunScratch<M> {
        self.scratch
            .take()
            .unwrap_or_else(|| RunScratch::new(&self.plan))
    }

    /// Borrows this runner until the returned run is dropped.
    pub fn start<'a>(&'a mut self, inputs: RunInputs<M>) -> Result<RunnerRun<'a, M>, StartError> {
        let mut scratch = self.take_scratch();
        scratch.reset(&self.plan);
        if let Err(error) = validate_run_inputs_into(&self.plan, inputs, &mut scratch.external) {
            self.scratch = Some(scratch);
            return Err(error);
        }
        let run = GraphRun::new(
            Arc::clone(&self.plan),
            scratch,
            None,
            self.dispatcher.clone(),
        );
        Ok(RunnerRun { run, runner: self })
    }
    /// Starts a borrowed runner future.
    pub fn execute<'a>(&'a mut self, inputs: RunInputs<M>) -> RunnerRun<'a, M> {
        let mut scratch = self.take_scratch();
        scratch.reset(&self.plan);
        let start_error = validate_run_inputs_into(&self.plan, inputs, &mut scratch.external).err();
        let run = GraphRun::new(
            Arc::clone(&self.plan),
            scratch,
            start_error,
            self.dispatcher.clone(),
        );
        RunnerRun { run, runner: self }
    }
    /// Releases cached per-run capacity.
    ///
    /// This does not affect the immutable compiled plan or dispatcher. An
    /// active [`RunnerRun`] exclusively borrows the runner, so its storage has
    /// already been returned before this method can be called.
    pub fn trim(&mut self) {
        self.scratch = None;
    }
}
/// Future borrowing a [`GraphRunner`] exclusively.
pub struct RunnerRun<'a, M: Mode> {
    run: GraphRun<M>,
    runner: &'a mut GraphRunner<M>,
}
impl<'a, M: Mode> RunnerRun<'a, M> {
    /// Returns run control.
    pub fn control(&self) -> RunControl<M> {
        self.run.control()
    }
}
impl<'a, M: Mode> Future for RunnerRun<'a, M> {
    type Output = Result<RunReport<M>, ExecuteError<M>>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().run).poll(cx)
    }
}
impl<'a, M: Mode> Unpin for RunnerRun<'a, M> {}

impl<'a, M: Mode> Drop for RunnerRun<'a, M> {
    fn drop(&mut self) {
        let scratch = self.run.take_scratch_for_reuse();
        debug_assert!(self.runner.scratch.is_none());
        self.runner.scratch = Some(scratch);
    }
}
