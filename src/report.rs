//! Completed-run observations, node failures, and retained Active target outputs.
//! output borrows a retained value; take_output transfers that shared ownership.

use crate::{
    error::{NodeError, OutputAccessError},
    handles::{NodeId, OutputSlot, RunId, VersionId},
    mode::{Mode, ValueFor},
    value::Shared,
};
use std::marker::PhantomData;

/// Lifecycle state of a selected node, independent of task completion order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NodeStatus {
    /// Waiting for its input dependencies.
    Pending,
    /// All inputs have resolved and the task may start.
    Ready,
    /// The task is executing or its Future is pending.
    Running,
    /// All declared outputs were committed together.
    Succeeded,
    /// The task failed without publishing outputs.
    Failed,
    /// Cancellation won before this node committed.
    Cancelled,
    /// An upstream dependency did not succeed.
    Blocked,
}
/// Final observations and strong ownership of successful Active target outputs.
///
/// Retaining this report can extend resource-handle lifetimes. Use
/// [`Self::take_output`] to transfer ownership; cancellation never revokes an
/// output committed before it took effect. Query operations are currently stubs.
pub struct RunReport<M: Mode> {
    failures: Vec<NodeFailure<M>>,
    _mode: PhantomData<M>,
}
impl<M: Mode> RunReport<M> {
    /// Identifies the immutable version that produced this report.
    pub fn version_id(&self) -> VersionId {
        unimplemented!()
    }
    /// Identifies this execution independently of other runs of the same version.
    pub fn run_id(&self) -> RunId {
        unimplemented!()
    }
    /// Returns a selected node's final state, or None when it is not in this run.
    pub fn status(&self, _node: NodeId) -> Option<NodeStatus> {
        unimplemented!()
    }
    /// Returns one direct failed dependency of a Blocked node, in stable order.
    pub fn blocked_by(&self, _node: NodeId) -> Option<NodeId> {
        unimplemented!()
    }
    /// Iterates all task failures in deterministic node order.
    pub fn failures(&self) -> impl ExactSizeIterator<Item = &NodeFailure<M>> {
        self.failures.iter()
    }
    /// Borrows a successful Active target's value without transferring ownership.
    ///
    /// The handle is checked against this report's version, not the edited graph.
    /// Foreign, stale, non-target, unsuccessful, and taken outputs have distinct errors.
    pub fn output<T: ValueFor<M>>(
        &self,
        _output: OutputSlot<T>,
    ) -> Result<&Shared<T, M>, OutputAccessError> {
        unimplemented!()
    }
    /// Moves a target's Shared ownership out of the report without copying T.
    /// Subsequent reads or takes return OutputTaken; other outputs are unaffected.
    pub fn take_output<T: ValueFor<M>>(
        &mut self,
        _output: OutputSlot<T>,
    ) -> Result<Shared<T, M>, OutputAccessError> {
        unimplemented!()
    }
}
/// A node identity paired with its structured failure and optional source error.
pub struct NodeFailure<M: Mode> {
    /// Node that failed in this run's immutable version.
    pub node: NodeId,
    /// Failure classification and application error source, when available.
    pub error: NodeError<M>,
}
