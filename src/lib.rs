//! Typed dependency graphs for synchronous and asynchronous tasks.
//!
//! Version 0.5.1 adds a measured direct path for synchronous task invocations
//! while retaining the executor-neutral async and external-dispatch paths.
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
    CancellationToken, Cancelled, GraphRun, GraphRunner, NodeDispatcher, NodeJob, RunControl,
    RunInput, RunInputs, RunnerRun,
};
pub use schema::{
    BoundSchema, Cardinality, InputSpec, OutputSpec, Presence, Schema, SchemaBuilder,
};
pub use task::{LocalTaskResult, SendTaskResult, Task, TaskContext};
pub use value::{NodeInputs, NodeOutputs, Shared};
