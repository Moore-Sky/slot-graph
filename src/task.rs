//! Repeatable synchronous/asynchronous task factories and per-invocation context.
//! Factories use Fn because each version can start multiple independent runs.
//!
//! A SendMode factory must return a Send Future, even if its captures are Send:
//! ```compile_fail
//! use slot_graph::{Graph, SendMode, schema, outputs};
//! use std::rc::Rc;
//! let mut graph = Graph::<SendMode>::new();
//! graph.add_async("local_future", schema! { () -> () }, |_, _| async {
//!     let local = Rc::new(1);
//!     std::future::pending::<()>().await;
//!     drop(local);
//!     Ok(outputs! {})
//! }).unwrap();
//! ```

use crate::{
    error::NodeError,
    handles::NodeId,
    mode::{Local, Mode, SendMode},
    runtime::CancellationToken,
    value::{NodeInputs, NodeOutputs},
};
use std::{future::Future, marker::PhantomData};

/// Per-invocation identity and cooperative cancellation access.
/// Its cancellation token can be moved into the task Future.
pub struct TaskContext<M: Mode> {
    node: NodeId,
    cancellation: CancellationToken<M>,
}
impl<M: Mode> TaskContext<M> {
    /// Identifies the node whose factory produced this task invocation.
    pub fn node_id(&self) -> NodeId {
        self.node
    }
    /// Clones the owned token associated with this run.
    pub fn cancellation(&self) -> CancellationToken<M> {
        self.cancellation.clone()
    }
}
/// Result returned by a Local task or its Future.
pub type LocalTaskResult = Result<NodeOutputs<Local>, NodeError<Local>>;
/// Result returned by a SendMode task or its Future.
pub type SendTaskResult = Result<NodeOutputs<SendMode>, NodeError<SendMode>>;

/// An owned, repeatable task factory used by `replace_task`.
pub struct Task<M: Mode> {
    _mode: PhantomData<M>,
}
impl Task<Local> {
    /// Adapts a repeatable synchronous factory, including thread-local captures.
    /// Factory storage is currently unimplemented.
    pub fn sync<F>(_task: F) -> Self
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> LocalTaskResult + 'static,
    {
        let _ = _task;
        unimplemented!()
    }
    /// Adapts a factory producing a fresh, possibly non-Send Future per run.
    /// Factory storage is currently unimplemented.
    pub fn asynchronous<F, Fut>(_task: F) -> Self
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> Fut + 'static,
        Fut: Future<Output = LocalTaskResult> + 'static,
    {
        let _ = _task;
        unimplemented!()
    }
}
impl Task<SendMode> {
    /// Adapts a repeatable Send + Sync synchronous factory. Currently a stub.
    pub fn sync<F>(_task: F) -> Self
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> SendTaskResult
            + Send
            + Sync
            + 'static,
    {
        let _ = _task;
        unimplemented!()
    }
    /// Adapts a Send + Sync factory producing a fresh Send Future per run.
    /// Factory storage is currently unimplemented.
    pub fn asynchronous<F, Fut>(_task: F) -> Self
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = SendTaskResult> + Send + 'static,
    {
        let _ = _task;
        unimplemented!()
    }
}
