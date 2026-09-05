//! Editable graph declarations, atomic connection edits, and compile entry points.
//! Editing never changes previously compiled execution versions.
//!
//! ```compile_fail
//! use slot_graph::{schema, Graph, SendMode};
//! use std::rc::Rc;
//! let captured = Rc::new(());
//! let mut graph = Graph::<SendMode>::new();
//! let _ = graph.add_sync("not-send", schema! { () -> () }, move |_, _| {
//!     let _ = &captured;
//!     Ok(slot_graph::outputs! {})
//! });
//! ```
//!
//! ```compile_fail
//! use slot_graph::{schema, Graph, Local};
//! let mut count = 0;
//! let mut graph = Graph::<Local>::new();
//! let _ = graph.add_sync("not-fn", schema! { () -> () }, move |_, _| {
//!     count += 1;
//!     Ok(slot_graph::outputs! {})
//! });
//! ```

use crate::value::NodeInputs;
use crate::{
    compiled::ExecutionGraphVersion,
    error::{CompileError, EditError},
    handles::*,
    mode::{Local, Mode, SendMode, ValueFor},
    runtime::RunInput,
    schema::Schema,
    task::*,
};
use std::{
    future::Future,
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};

/// Mutable declaration graph for one execution mode.
///
/// Editing requires `&mut self`; [`compile`](Self::compile) snapshots the
/// current declaration into an immutable version. Edits never change already
/// compiled versions or their runs.
pub struct Graph<M: Mode> {
    id: GraphId,
    _mode: PhantomData<M>,
}
impl<M: Mode> Graph<M> {
    /// Returns this graph's process-local identity.
    pub fn graph_id(&self) -> GraphId {
        self.id
    }
    /// Removes a node, its incident edges, and its active-target marker.
    ///
    /// The returned report lists edge identities made stale by the removal.
    pub fn remove_node(&mut self, _node: NodeId) -> Result<RemoveNodeReport, EditError> {
        unimplemented!()
    }
    /// Changes a node's unique lookup name without changing its identity or
    /// existing edges.
    pub fn rename_node(
        &mut self,
        _node: NodeId,
        _name: impl Into<String>,
    ) -> Result<(), EditError> {
        unimplemented!()
    }
    /// Replaces a node's repeatable task factory while preserving its schema
    /// and all existing edges.
    pub fn replace_task(&mut self, _node: NodeId, _task: Task<M>) -> Result<(), EditError> {
        unimplemented!()
    }
    /// Replaces a node's schema and task atomically.
    ///
    /// Compatible edges retain their identity and order; incompatible edges
    /// and external-input bindings are listed in the returned report. All old
    /// typed slot handles for the node become stale after success.
    pub fn replace_schema(
        &mut self,
        _node: NodeId,
        _schema: Schema,
        _task: Task<M>,
    ) -> Result<SchemaReplaceReport, EditError> {
        unimplemented!()
    }
    /// Resolves a typed, generation-checked input handle by node and name.
    ///
    /// This fails for foreign or stale nodes, unknown slots, or a type mismatch.
    pub fn input<T: ValueFor<M>>(
        &self,
        node: NodeId,
        name: &str,
    ) -> Result<InputSlot<T>, EditError> {
        let _ = (node, name);
        unimplemented!()
    }
    /// Resolves a typed, generation-checked output handle by node and name.
    ///
    /// This fails for foreign or stale nodes, unknown slots, or a type mismatch.
    pub fn output<T: ValueFor<M>>(
        &self,
        node: NodeId,
        name: &str,
    ) -> Result<OutputSlot<T>, EditError> {
        let _ = (node, name);
        unimplemented!()
    }
    /// Adds one explicit output-to-input edge.
    ///
    /// The operation validates graph ownership, slot generations, direction,
    /// exact type equality, duplicate edges, and input cardinality immediately.
    pub fn connect(
        &mut self,
        _output: impl Into<OutputSlotSelector>,
        _input: impl Into<InputSlotSelector>,
    ) -> Result<EdgeId, EditError> {
        unimplemented!()
    }
    /// Removes exactly one edge by identity.
    ///
    /// The edge identity becomes stale after a successful removal.
    pub fn disconnect(&mut self, _edge: EdgeId) -> Result<(), EditError> {
        unimplemented!()
    }
    /// Atomically changes an edge's output source.
    ///
    /// The new source is validated with the same direction, type, duplicate,
    /// and cardinality rules as [`connect`](Self::connect); on error the old
    /// edge remains unchanged.
    pub fn reconnect(
        &mut self,
        _edge: EdgeId,
        _output: impl Into<OutputSlotSelector>,
    ) -> Result<EdgeId, EditError> {
        unimplemented!()
    }
    /// Plans and atomically applies deterministic convenience connections
    /// between two nodes.
    ///
    /// Explicit [`connect`](Self::connect) remains authoritative. The report
    /// describes created edges and inputs with no automatic match.
    pub fn connect_nodes(
        &mut self,
        _source: NodeId,
        _target: NodeId,
    ) -> Result<AutoConnectReport, EditError> {
        unimplemented!()
    }
    /// Exposes an otherwise unproduced input as a per-run external entry.
    ///
    /// An exposed input cannot also have a normal producer. Compiled versions
    /// freeze their own accepted input definitions.
    pub fn expose_input<T: ValueFor<M>>(
        &mut self,
        input: impl Into<InputSlotSelector>,
    ) -> Result<RunInput<T, M>, EditError> {
        let _ = input.into();
        unimplemented!()
    }
    /// Adds or removes a compilation target.
    ///
    /// Active nodes are roots, not enable flags: compilation includes all of
    /// their upstream dependencies.
    pub fn set_active(
        &mut self,
        _node: impl Into<NodeSelector>,
        _active: bool,
    ) -> Result<(), EditError> {
        unimplemented!()
    }
    /// Fully compiles the current active reverse closure into an immutable
    /// execution version.
    ///
    /// Compilation does not publish a current version and does not mutate this
    /// declaration graph. Global validation is limited to the selected closure.
    pub fn compile(&self) -> Result<ExecutionGraphVersion<M>, CompileError> {
        unimplemented!()
    }
}
impl Graph<Local> {
    /// Creates an empty graph that accepts local values and `!Send` futures.
    pub fn new() -> Self {
        Self::new_inner()
    }
    /// Adds a repeatable synchronous local task with its schema.
    pub fn add_sync<F>(
        &mut self,
        name: impl Into<String>,
        schema: Schema,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> LocalTaskResult + 'static,
    {
        let _ = (name, schema, task);
        unimplemented!()
    }
    /// Adds a repeatable asynchronous local task with its schema.
    pub fn add_async<F, Fut>(
        &mut self,
        name: impl Into<String>,
        schema: Schema,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> Fut + 'static,
        Fut: Future<Output = LocalTaskResult> + 'static,
    {
        let _ = (name, schema, task);
        unimplemented!()
    }
}
impl Graph<SendMode> {
    /// Creates an empty graph whose factories, values, and futures satisfy
    /// cross-thread bounds.
    pub fn new() -> Self {
        Self::new_inner()
    }
    /// Adds a repeatable synchronous task whose factory is `Send + Sync`.
    pub fn add_sync<F>(
        &mut self,
        name: impl Into<String>,
        schema: Schema,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> SendTaskResult
            + Send
            + Sync
            + 'static,
    {
        let _ = (name, schema, task);
        unimplemented!()
    }
    /// Adds a repeatable asynchronous task whose factory and future are
    /// cross-thread safe.
    pub fn add_async<F, Fut>(
        &mut self,
        name: impl Into<String>,
        schema: Schema,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = SendTaskResult> + Send + 'static,
    {
        let _ = (name, schema, task);
        unimplemented!()
    }
}
/// Creates an empty local graph.
impl Default for Graph<Local> {
    fn default() -> Self {
        Self::new()
    }
}

/// Creates an empty sendable graph.
impl Default for Graph<SendMode> {
    fn default() -> Self {
        Self::new()
    }
}

impl<M: Mode> Graph<M> {
    fn new_inner() -> Self {
        Self {
            id: GraphId(NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed)),
            _mode: PhantomData,
        }
    }
}
static NEXT_GRAPH_ID: AtomicU64 = AtomicU64::new(1);
/// Result of a successful [`Graph::connect_nodes`] operation.
///
/// All listed edges are applied atomically in deterministic schema order.
#[derive(Clone, Debug, Default)]
pub struct AutoConnectReport {
    /// Edge identities created by this operation.
    pub edges: Vec<EdgeId>,
    /// Required inputs for which no automatic connection was found.
    pub unmatched_required: Vec<InputSlotSelector>,
    /// Optional inputs for which no automatic connection was found.
    pub unmatched_optional: Vec<InputSlotSelector>,
}
/// Result of removing a node from a declaration graph.
#[derive(Clone, Debug, Default)]
pub struct RemoveNodeReport {
    /// Incident edge identities removed with the node.
    pub removed_edges: Vec<EdgeId>,
}
/// Result of a successful [`Graph::replace_schema`] operation.
#[derive(Clone, Debug, Default)]
pub struct SchemaReplaceReport {
    /// Incompatible edges removed by the replacement and now stale.
    pub removed_edges: Vec<EdgeId>,
    /// External input bindings removed because their full input contract
    /// changed.
    pub removed_inputs: Vec<InputSlotSelector>,
}
