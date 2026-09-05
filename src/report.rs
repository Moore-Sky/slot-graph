//! Completed-run observations, node failures, and retained Active target outputs.
//! output borrows a retained value; take_output transfers that shared ownership.

use crate::{
    error::{ErrorContext, NodeError, OutputAccessError, OutputAccessErrorKind},
    handles::{GraphId, NodeId, OutputSlot, RunId, SlotId, VersionId},
    mode::{Mode, ValueFor},
    value::{Shared, StoredValue},
};

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

/// One selected node's final report data.
///
/// The executor builds these records in the compiled plan's stable node order.
/// `outputs` describes the version-local output handles that remain valid for
/// this node, including outputs that were not selected as report targets.
#[doc(hidden)]
pub(crate) struct ReportNode {
    pub(crate) node: NodeId,
    pub(crate) status: NodeStatus,
    pub(crate) blocked_by: Option<NodeId>,
    pub(crate) outputs: Vec<(SlotId, u64)>,
}

/// One Active target output retained by a completed report.
///
/// A missing value denotes an unsuccessful target. Once moved by
/// [`RunReport::take_output`], `taken` distinguishes that state from an
/// unavailable failed or cancelled target.
#[doc(hidden)]
pub(crate) struct TargetOutput {
    pub(crate) node: NodeId,
    pub(crate) slot: SlotId,
    pub(crate) generation: u64,
    pub(crate) value: Option<StoredValue>,
    taken: bool,
}

impl TargetOutput {
    /// Creates a target entry whose value was committed successfully.
    pub(crate) fn available(
        node: NodeId,
        slot: SlotId,
        generation: u64,
        value: StoredValue,
    ) -> Self {
        Self {
            node,
            slot,
            generation,
            value: Some(value),
            taken: false,
        }
    }

    /// Creates a target entry whose node did not commit that output.
    pub(crate) fn unavailable(node: NodeId, slot: SlotId, generation: u64) -> Self {
        Self {
            node,
            slot,
            generation,
            value: None,
            taken: false,
        }
    }
}
/// Final observations and strong ownership of successful Active target outputs.
///
/// Retaining this report can extend resource-handle lifetimes. Use
/// [`Self::take_output`] to transfer ownership; cancellation never revokes an
/// output committed before it took effect.
pub struct RunReport<M: Mode> {
    graph: GraphId,
    version: VersionId,
    run: RunId,
    nodes: Vec<ReportNode>,
    failures: Vec<NodeFailure<M>>,
    targets: Vec<TargetOutput>,
}
impl<M: Mode> RunReport<M> {
    /// Assembles a completed report from stable executor state.
    ///
    /// `nodes` and `failures` must already be in the compiled plan's stable
    /// node order. `targets` contains exactly the outputs selected as Active
    /// report targets; it may contain unavailable entries for failed or
    /// cancelled nodes.
    pub(crate) fn new(
        graph: GraphId,
        version: VersionId,
        run: RunId,
        nodes: Vec<ReportNode>,
        failures: Vec<NodeFailure<M>>,
        targets: Vec<TargetOutput>,
    ) -> Self {
        Self {
            graph,
            version,
            run,
            nodes,
            failures,
            targets,
        }
    }

    /// Identifies the immutable version that produced this report.
    pub fn version_id(&self) -> VersionId {
        self.version
    }
    /// Identifies this execution independently of other runs of the same version.
    pub fn run_id(&self) -> RunId {
        self.run
    }
    /// Returns a selected node's final state, or None when it is not in this run.
    pub fn status(&self, node: NodeId) -> Option<NodeStatus> {
        self.nodes
            .iter()
            .find(|entry| entry.node == node)
            .map(|entry| entry.status)
    }
    /// Returns one direct failed dependency of a Blocked node, in stable order.
    pub fn blocked_by(&self, node: NodeId) -> Option<NodeId> {
        self.nodes
            .iter()
            .find(|entry| entry.node == node)
            .and_then(|entry| entry.blocked_by)
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
        output: OutputSlot<T>,
    ) -> Result<&Shared<T, M>, OutputAccessError> {
        let index = self.target_index(output)?;
        let target = &self.targets[index];
        match target.value.as_ref() {
            Some(value) => value
                .shared::<T, M>()
                .ok_or_else(|| self.output_error(OutputAccessErrorKind::TypeMismatch, output)),
            None if target.taken => {
                Err(self.output_error(OutputAccessErrorKind::OutputTaken, output))
            }
            None => Err(self.output_error(OutputAccessErrorKind::OutputUnavailable, output)),
        }
    }
    /// Moves a target's Shared ownership out of the report without copying T.
    /// Subsequent reads or takes return OutputTaken; other outputs are unaffected.
    pub fn take_output<T: ValueFor<M>>(
        &mut self,
        output: OutputSlot<T>,
    ) -> Result<Shared<T, M>, OutputAccessError> {
        let index = self.target_index(output)?;
        let target = &mut self.targets[index];
        if target
            .value
            .as_ref()
            .is_some_and(|value| value.shared::<T, M>().is_none())
        {
            return Err(Self::output_error_for(
                OutputAccessErrorKind::TypeMismatch,
                output,
            ));
        }
        match target.value.take() {
            Some(value) => {
                // The borrowed downcast above proves this conversion. Keeping
                // the impossible branch explicit avoids an unsound type cast if
                // an internal value implementation is changed later.
                let value = value.into_shared::<T, M>().ok_or_else(|| {
                    Self::output_error_for(OutputAccessErrorKind::TypeMismatch, output)
                })?;
                target.taken = true;
                Ok(value)
            }
            None if target.taken => Err(Self::output_error_for(
                OutputAccessErrorKind::OutputTaken,
                output,
            )),
            None => Err(Self::output_error_for(
                OutputAccessErrorKind::OutputUnavailable,
                output,
            )),
        }
    }

    fn target_index<T: ValueFor<M>>(
        &self,
        output: OutputSlot<T>,
    ) -> Result<usize, OutputAccessError> {
        let (node, slot, generation) = output.parts();
        if node.graph() != self.graph {
            return Err(self.output_error(OutputAccessErrorKind::ForeignHandle, output));
        }

        let Some(report_node) = self.nodes.iter().find(|entry| entry.node == node) else {
            return Err(self.output_error(OutputAccessErrorKind::NotCollected, output));
        };
        if !report_node
            .outputs
            .iter()
            .any(|&(known_slot, known_generation)| {
                known_slot == slot && known_generation == generation
            })
        {
            return Err(self.output_error(OutputAccessErrorKind::StaleSlotHandle, output));
        }

        self.targets
            .iter()
            .position(|target| {
                target.node == node && target.slot == slot && target.generation == generation
            })
            .ok_or_else(|| self.output_error(OutputAccessErrorKind::NotCollected, output))
    }

    fn output_error<T: ?Sized>(
        &self,
        kind: OutputAccessErrorKind,
        output: OutputSlot<T>,
    ) -> OutputAccessError {
        Self::output_error_for(kind, output)
    }

    fn output_error_for<T: ?Sized>(
        kind: OutputAccessErrorKind,
        output: OutputSlot<T>,
    ) -> OutputAccessError {
        let (node, slot, _) = output.parts();
        OutputAccessError::new(
            kind,
            ErrorContext {
                graph: Some(node.graph()),
                node: Some(node),
                slot: Some(slot),
                ..ErrorContext::default()
            },
        )
    }
}
/// A node identity paired with its structured failure and optional source error.
pub struct NodeFailure<M: Mode> {
    /// Node that failed in this run's immutable version.
    pub node: NodeId,
    /// Failure classification and application error source, when available.
    pub error: NodeError<M>,
}
