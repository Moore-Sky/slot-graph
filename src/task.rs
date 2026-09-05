//! Repeatable synchronous/asynchronous task factories and per-invocation context.
//!
//! A factory is an `Fn`, rather than an `FnOnce`, because one compiled version
//! may start many independent runs. Each invocation produces its own future.

use crate::{
    error::NodeError,
    handles::NodeId,
    mode::{Local, Mode, SendMode},
    runtime::CancellationToken,
    value::{NodeInputs, NodeOutputs},
};
use std::{future::Future, marker::PhantomData, pin::Pin, sync::Arc};

/// Per-invocation identity and cooperative cancellation access.
/// Its cancellation token can be moved into the task Future.
pub struct TaskContext<M: Mode> {
    node: NodeId,
    cancellation: CancellationToken<M>,
}
impl<M: Mode> TaskContext<M> {
    pub(crate) fn new(node: NodeId, cancellation: CancellationToken<M>) -> Self {
        Self { node, cancellation }
    }
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
pub(crate) type TaskResult<M> = Result<NodeOutputs<M>, NodeError<M>>;

/// One fresh asynchronous future returned from a task factory.
///
/// The erased future keeps the public `Task` representation independent of a
/// concrete async type. Only the SendMode constructors can create a
/// `TaskFuture<SendMode>`, and they require the original future to be `Send`.
pub(crate) struct TaskFuture<M: Mode> {
    future: Pin<Box<dyn Future<Output = TaskResult<M>> + 'static>>,
    _mode: PhantomData<M>,
}
impl<M: Mode> Unpin for TaskFuture<M> {}
impl<M: Mode> Future for TaskFuture<M> {
    type Output = TaskResult<M>;
    fn poll(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.get_mut().future.as_mut().poll(cx)
    }
}

/// One invocation of a task factory.
///
/// Synchronous factories return their result directly. This keeps the common
/// inline path from allocating and polling a `ready` future solely to adapt to
/// the asynchronous representation. Asynchronous factories retain the erased
/// future required by the executor-neutral runtime boundary.
pub(crate) enum TaskInvocation<M: Mode> {
    Sync(TaskResult<M>),
    Async(TaskFuture<M>),
}

type Factory<M> = dyn Fn(TaskContext<M>, NodeInputs<M>) -> TaskInvocation<M> + 'static;

/// An owned, repeatable task factory used by graph registration and replacement.
pub struct Task<M: Mode> {
    factory: Arc<Factory<M>>,
    _mode: PhantomData<M>,
}
impl<M: Mode> Clone for Task<M> {
    fn clone(&self) -> Self {
        Self {
            factory: Arc::clone(&self.factory),
            _mode: PhantomData,
        }
    }
}
impl<M: Mode> Task<M> {
    /// Invokes the factory exactly once.
    pub(crate) fn invoke(
        &self,
        context: TaskContext<M>,
        inputs: NodeInputs<M>,
    ) -> TaskInvocation<M> {
        (self.factory)(context, inputs)
    }
}

impl Task<Local> {
    /// Adapts a repeatable synchronous factory, including thread-local captures.
    pub fn sync<F>(task: F) -> Self
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> LocalTaskResult + 'static,
    {
        Self {
            factory: Arc::new(move |context, inputs| TaskInvocation::Sync(task(context, inputs))),
            _mode: PhantomData,
        }
    }
    /// Adapts a factory producing a fresh, possibly non-Send Future per run.
    pub fn asynchronous<F, Fut>(task: F) -> Self
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> Fut + 'static,
        Fut: Future<Output = LocalTaskResult> + 'static,
    {
        Self {
            factory: Arc::new(move |context, inputs| {
                TaskInvocation::Async(TaskFuture {
                    future: Box::pin(task(context, inputs)),
                    _mode: PhantomData,
                })
            }),
            _mode: PhantomData,
        }
    }
}
impl Task<SendMode> {
    /// Adapts a repeatable Send + Sync synchronous factory.
    pub fn sync<F>(task: F) -> Self
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> SendTaskResult
            + Send
            + Sync
            + 'static,
    {
        Self {
            factory: Arc::new(move |context, inputs| TaskInvocation::Sync(task(context, inputs))),
            _mode: PhantomData,
        }
    }
    /// Adapts a Send + Sync factory producing a fresh Send Future per run.
    pub fn asynchronous<F, Fut>(task: F) -> Self
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = SendTaskResult> + Send + 'static,
    {
        Self {
            factory: Arc::new(move |context, inputs| {
                TaskInvocation::Async(TaskFuture {
                    future: Box::pin(task(context, inputs)),
                    _mode: PhantomData,
                })
            }),
            _mode: PhantomData,
        }
    }
}

// The type-erased fields do not carry auto-traits. Only this private marker
// opts a task back into cross-thread transport; its sole implementation is the
// mode whose constructors require a Send + Sync factory and a Send future.
trait SendModeTask: Mode + Send + Sync {}
impl SendModeTask for SendMode {}

unsafe impl<M: SendModeTask> Send for Task<M> {}
unsafe impl<M: SendModeTask> Sync for Task<M> {}
unsafe impl<M: SendModeTask> Send for TaskFuture<M> {}
unsafe impl<M: SendModeTask> Send for TaskInvocation<M> {}
