//! Immutable compiled versions that can start isolated runs or reusable runners.
//! Compilation and execution are API stubs in this revision.
//!
//! Send execution rejects a dispatcher with thread-local state:
//! ```compile_fail
//! use slot_graph::{DispatchError, Graph, NodeJob, RunInputs, SendMode};
//! use std::rc::Rc;
//! let version = Graph::<SendMode>::new().compile().unwrap();
//! let state = Rc::new(());
//! let dispatcher = move |_job: NodeJob<SendMode>| {
//!     let _ = &state;
//!     Ok::<(), DispatchError>(())
//! };
//! let _run = version.execute_on(RunInputs::new(), dispatcher);
//! ```

use crate::{
    error::StartError,
    handles::{GraphId, VersionId},
    mode::{Local, Mode, SendMode},
    runtime::{GraphRun, GraphRunner, NodeDispatcher, RunInputs},
};
use std::marker::PhantomData;

/// Immutable plan produced by compiling a declaration graph.
pub struct ExecutionGraphVersion<M: Mode> {
    graph: GraphId,
    _mode: PhantomData<M>,
}
impl<M: Mode> ExecutionGraphVersion<M> {
    /// Identifies this immutable compilation independently of other versions.
    pub fn version_id(&self) -> VersionId {
        unimplemented!()
    }
    /// Returns the declaration graph identity captured by this version.
    pub fn graph_id(&self) -> GraphId {
        self.graph
    }
    /// Validates inputs and creates an owned run; currently a stub.
    pub fn start(&self, _inputs: RunInputs<M>) -> Result<GraphRun<M>, StartError> {
        unimplemented!()
    }
    /// Creates an execution future which returns start or execution errors; currently a stub.
    pub fn execute(&self, _inputs: RunInputs<M>) -> GraphRun<M> {
        unimplemented!()
    }
    /// Creates independent reusable runner storage; currently a stub.
    pub fn runner(&self) -> GraphRunner<M> {
        unimplemented!()
    }
}

impl ExecutionGraphVersion<Local> {
    /// Validates inputs and starts a run whose Ready nodes use a local dispatcher.
    ///
    /// The dispatcher and NodeJob may be !Send, but the dispatcher is owned and
    /// 'static. For example, an async-runtime adapter can own `Rc<LocalDomain>`
    /// while another `Rc` clone drives that domain on its owner thread. A borrowed
    /// `&LocalDomain` adapter is not accepted. Currently unimplemented.
    pub fn start_on<D>(
        &self,
        _inputs: RunInputs<Local>,
        _dispatcher: D,
    ) -> Result<GraphRun<Local>, StartError>
    where
        D: NodeDispatcher<Local>,
    {
        unimplemented!()
    }

    /// Creates a local dispatcher-backed execution future. Start failures are
    /// returned by the future like ordinary execute. Currently unimplemented.
    pub fn execute_on<D>(&self, _inputs: RunInputs<Local>, _dispatcher: D) -> GraphRun<Local>
    where
        D: NodeDispatcher<Local>,
    {
        unimplemented!()
    }

    /// Creates reusable local run storage using the supplied owner-thread dispatcher.
    /// Currently unimplemented.
    pub fn runner_on<D>(&self, _dispatcher: D) -> GraphRunner<Local>
    where
        D: NodeDispatcher<Local>,
    {
        unimplemented!()
    }
}

impl ExecutionGraphVersion<SendMode> {
    /// Validates inputs and starts a run whose Ready nodes may execute in parallel.
    ///
    /// The dispatcher is executor-neutral. Send + Sync is an intentionally
    /// conservative adapter contract even though one GraphRun is never polled
    /// concurrently. Each submitted NodeJob is Send. Currently unimplemented.
    pub fn start_on<D>(
        &self,
        _inputs: RunInputs<SendMode>,
        _dispatcher: D,
    ) -> Result<GraphRun<SendMode>, StartError>
    where
        D: NodeDispatcher<SendMode> + Send + Sync,
    {
        unimplemented!()
    }

    /// Creates a Send dispatcher-backed execution future. Spawning this
    /// GraphRun schedules orchestration; independently Ready NodeJobs are still
    /// submitted separately to the dispatcher. Currently unimplemented.
    pub fn execute_on<D>(&self, _inputs: RunInputs<SendMode>, _dispatcher: D) -> GraphRun<SendMode>
    where
        D: NodeDispatcher<SendMode> + Send + Sync,
    {
        unimplemented!()
    }

    /// Creates reusable Send run storage using the supplied external dispatcher.
    /// Currently unimplemented.
    pub fn runner_on<D>(&self, _dispatcher: D) -> GraphRunner<SendMode>
    where
        D: NodeDispatcher<SendMode> + Send + Sync,
    {
        unimplemented!()
    }
}
