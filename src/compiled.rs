//! Immutable compiled versions that can start isolated runs or reusable runners.
//! Compilation and execution are API stubs in this revision.

use crate::{
    error::StartError,
    handles::{GraphId, VersionId},
    mode::Mode,
    runtime::{GraphRun, GraphRunner, RunInputs},
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
