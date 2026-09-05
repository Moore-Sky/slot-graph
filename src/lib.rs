//! Typed dependency graphs for synchronous and asynchronous tasks.
//!
//! This v0.3.1 revision defines the public API and its contract tests. Graph
//! editing, compilation, and execution deliberately use `unimplemented!()`.
//! Schema descriptors and the Shared ownership wrapper are usable.
//!
//! Start with [Graph], declare [Schema] inputs and outputs, compile an
//! [ExecutionGraphVersion], then drive a [GraphRun] with your host executor.
//! See the numbered examples and `documents/design.md` for complete scenarios.

#![deny(missing_docs)]

pub mod compiled;
pub mod error;
pub mod graph;
pub mod handles;
mod macros;
pub mod mode;
pub mod report;
pub mod runtime;
pub mod schema;
pub mod task;
pub mod value;

pub use compiled::ExecutionGraphVersion;
pub use error::*;
pub use graph::{AutoConnectReport, Graph, RemoveNodeReport, SchemaReplaceReport};
pub use handles::*;
pub use mode::{Local, Mode, SendMode, UserErrorFor, ValueFor};
pub use report::{NodeFailure, NodeStatus, RunReport};
pub use runtime::{
    CancellationToken, Cancelled, GraphRun, GraphRunner, RunControl, RunInput, RunInputs, RunnerRun,
};
pub use schema::{
    BoundSchema, Cardinality, InputSpec, OutputSpec, Presence, Schema, SchemaBuilder,
};
pub use task::{LocalTaskResult, SendTaskResult, Task, TaskContext};
pub use value::{NodeInputs, NodeOutputs, Shared};
