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
    error::{CompileError, EditError, EditErrorKind, ErrorContext},
    handles::*,
    mode::{Local, Mode, SendMode, ValueFor},
    runtime::RunInput,
    schema::{BoundSchema, Cardinality, InputSpec, OutputSpec, Presence},
    task::*,
};
use std::{
    collections::{HashMap, HashSet},
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
    pub(crate) nodes: Vec<Option<NodeRecord<M>>>,
    pub(crate) edges: Vec<Option<EdgeRecord>>,
    names: HashMap<String, NodeId>,
    pub(crate) exposed: Vec<ExposedInput>,
    next_node: u64,
    next_edge: u64,
    next_binding: u64,
    _mode: PhantomData<M>,
}

pub(crate) struct NodeRecord<M: Mode> {
    pub(crate) id: NodeId,
    pub(crate) name: String,
    pub(crate) schema: BoundSchema,
    pub(crate) schema_generation: u64,
    pub(crate) task: Task<M>,
    pub(crate) active: bool,
}

#[derive(Clone)]
pub(crate) struct EdgeRecord {
    pub(crate) id: EdgeId,
    pub(crate) source_node: NodeId,
    pub(crate) output: SlotId,
    pub(crate) target_node: NodeId,
    pub(crate) input: SlotId,
}

#[derive(Clone)]
pub(crate) struct ExposedInput {
    pub(crate) node: NodeId,
    pub(crate) input: SlotId,
    pub(crate) binding: u64,
    pub(crate) value_type: SlotTypeId,
    pub(crate) presence: Presence,
    pub(crate) cardinality: Cardinality,
}

#[derive(Clone)]
struct ResolvedInput {
    node: NodeId,
    slot: SlotId,
    generation: u64,
    spec: InputSpec,
}

#[derive(Clone)]
struct ResolvedOutput {
    node: NodeId,
    slot: SlotId,
    spec: OutputSpec,
}
impl<M: Mode> Graph<M> {
    /// Returns this graph's process-local identity.
    pub fn graph_id(&self) -> GraphId {
        self.id
    }
    /// Removes a node, its incident edges, and its active-target marker.
    ///
    /// The returned report lists edge identities made stale by the removal.
    pub fn remove_node(&mut self, node: NodeId) -> Result<RemoveNodeReport, EditError> {
        let index = self.node_index(node)?;
        let name = self.nodes[index].as_ref().unwrap().name.clone();
        let mut removed_edges = Vec::new();
        for edge in &mut self.edges {
            if edge
                .as_ref()
                .is_some_and(|edge| edge.source_node == node || edge.target_node == node)
            {
                removed_edges.push(edge.as_ref().unwrap().id);
                *edge = None;
            }
        }
        self.exposed.retain(|binding| binding.node != node);
        self.names.remove(&name);
        self.nodes[index] = None;
        Ok(RemoveNodeReport { removed_edges })
    }
    /// Changes a node's unique lookup name without changing its identity or
    /// existing edges.
    pub fn rename_node(&mut self, node: NodeId, name: impl Into<String>) -> Result<(), EditError> {
        let index = self.node_index(node)?;
        let name = name.into();
        self.validate_node_name(&name, Some(node))?;
        let old = self.nodes[index].as_ref().unwrap().name.clone();
        self.names.remove(&old);
        self.names.insert(name.clone(), node);
        self.nodes[index].as_mut().unwrap().name = name;
        Ok(())
    }
    /// Replaces a node's repeatable task factory while preserving its schema
    /// and all existing edges.
    pub fn replace_task(&mut self, node: NodeId, task: Task<M>) -> Result<(), EditError> {
        let index = self.node_index(node)?;
        self.nodes[index].as_mut().unwrap().task = task;
        Ok(())
    }
    /// Replaces a node's schema and task atomically.
    ///
    /// Compatible edges retain their identity and order; incompatible edges
    /// and external-input bindings are listed in the returned report. All old
    /// typed slot handles for the node become stale after success.
    /// Task keys follow bound-layout identity instead: a fresh binding rejects
    /// old keys, whereas passing the same BoundSchema clone preserves them.
    /// Previously compiled versions retain their own layouts in either case.
    pub fn replace_schema(
        &mut self,
        node: NodeId,
        schema: impl Into<BoundSchema>,
        task: Task<M>,
    ) -> Result<SchemaReplaceReport, EditError> {
        let node_index = self.node_index(node)?;
        let schema = schema.into();
        schema.schema().validate()?;
        let old_schema = self.nodes[node_index].as_ref().unwrap().schema.clone();

        let compatible_input = |slot: SlotId, value_type: SlotTypeId| {
            schema
                .schema()
                .inputs
                .iter()
                .find(|candidate| candidate.id == slot && candidate.value_type == value_type)
        };
        let compatible_output = |slot: SlotId, value_type: SlotTypeId| {
            schema
                .schema()
                .outputs
                .iter()
                .find(|candidate| candidate.id == slot && candidate.value_type == value_type)
        };

        let mut removed_edges = Vec::new();
        let mut retained_per_input: HashMap<SlotId, usize> = HashMap::new();
        for edge in self.edges.iter().flatten() {
            let input_retained = if edge.target_node == node {
                let old = old_schema
                    .schema()
                    .inputs
                    .iter()
                    .find(|slot| slot.id == edge.input)
                    .expect("live edge input exists in current schema");
                compatible_input(edge.input, old.value_type).is_some()
            } else {
                true
            };
            let output_retained = if edge.source_node == node {
                let old = old_schema
                    .schema()
                    .outputs
                    .iter()
                    .find(|slot| slot.id == edge.output)
                    .expect("live edge output exists in current schema");
                compatible_output(edge.output, old.value_type).is_some()
            } else {
                true
            };
            let retained = input_retained && output_retained;
            if !retained {
                removed_edges.push(edge.id);
            } else if edge.target_node == node {
                *retained_per_input.entry(edge.input).or_default() += 1;
            }
        }
        for input in &schema.schema().inputs {
            if input.cardinality == Cardinality::One
                && retained_per_input.get(&input.id).copied().unwrap_or(0) > 1
            {
                return Err(self.edit_error(EditErrorKind::CardinalityOverflow, Some(node), None));
            }
        }

        let mut removed_inputs = Vec::new();
        for binding in self.exposed.iter().filter(|binding| binding.node == node) {
            let keep = schema.schema().inputs.iter().any(|candidate| {
                candidate.id == binding.input
                    && candidate.value_type == binding.value_type
                    && candidate.presence == binding.presence
                    && candidate.cardinality == binding.cardinality
            });
            if !keep {
                let old_name = old_schema
                    .schema()
                    .inputs
                    .iter()
                    .find(|input| input.id == binding.input)
                    .map(|input| input.name.clone())
                    .unwrap_or_default();
                removed_inputs.push(node.input(old_name));
            }
        }

        let removed_edge_set: HashSet<EdgeId> = removed_edges.iter().copied().collect();
        for edge in &mut self.edges {
            if edge
                .as_ref()
                .is_some_and(|edge| removed_edge_set.contains(&edge.id))
            {
                *edge = None;
            }
        }
        let removed_binding_keys: HashSet<SlotId> = self
            .exposed
            .iter()
            .filter(|binding| binding.node == node)
            .filter_map(|binding| {
                (!schema.schema().inputs.iter().any(|candidate| {
                    candidate.id == binding.input
                        && candidate.value_type == binding.value_type
                        && candidate.presence == binding.presence
                        && candidate.cardinality == binding.cardinality
                }))
                .then_some(binding.input)
            })
            .collect();
        self.exposed.retain(|binding| {
            binding.node != node || !removed_binding_keys.contains(&binding.input)
        });
        let record = self.nodes[node_index].as_mut().unwrap();
        record.schema = schema;
        record.task = task;
        record.schema_generation = record.schema_generation.wrapping_add(1);
        Ok(SchemaReplaceReport {
            removed_edges,
            removed_inputs,
        })
    }
    /// Resolves a typed, generation-checked input handle by node and name.
    ///
    /// This fails for foreign or stale nodes, unknown slots, or a type mismatch.
    pub fn input<T: ValueFor<M>>(
        &self,
        node: NodeId,
        name: &str,
    ) -> Result<InputSlot<T>, EditError> {
        let record = self.node(node)?;
        let spec = record
            .schema
            .schema()
            .inputs
            .iter()
            .find(|input| input.name == name)
            .ok_or_else(|| self.slot_error(EditErrorKind::UnknownSlotName, node, name))?;
        if spec.value_type != SlotTypeId::of::<T>() {
            return Err(self.slot_error(EditErrorKind::TypeMismatch, node, name));
        }
        Ok(InputSlot::new(node, spec.id, record.schema_generation))
    }
    /// Resolves a typed, generation-checked output handle by node and name.
    ///
    /// This fails for foreign or stale nodes, unknown slots, or a type mismatch.
    pub fn output<T: ValueFor<M>>(
        &self,
        node: NodeId,
        name: &str,
    ) -> Result<OutputSlot<T>, EditError> {
        let record = self.node(node)?;
        let spec = record
            .schema
            .schema()
            .outputs
            .iter()
            .find(|output| output.name == name)
            .ok_or_else(|| self.slot_error(EditErrorKind::UnknownSlotName, node, name))?;
        if spec.value_type != SlotTypeId::of::<T>() {
            return Err(self.slot_error(EditErrorKind::TypeMismatch, node, name));
        }
        Ok(OutputSlot::new(node, spec.id, record.schema_generation))
    }
    /// Adds one explicit output-to-input edge.
    ///
    /// The operation validates graph ownership, slot generations, direction,
    /// exact type equality, duplicate edges, and input cardinality immediately.
    pub fn connect(
        &mut self,
        output: impl Into<OutputSlotSelector>,
        input: impl Into<InputSlotSelector>,
    ) -> Result<EdgeId, EditError> {
        let output = self.resolve_output(output.into())?;
        let input = self.resolve_input(input.into())?;
        self.validate_connection(&output, &input, None)?;
        Ok(self.push_edge(output.node, output.slot, input.node, input.slot))
    }
    /// Removes exactly one edge by identity.
    ///
    /// The edge identity becomes stale after a successful removal.
    pub fn disconnect(&mut self, edge: EdgeId) -> Result<(), EditError> {
        let index = self.edge_index(edge)?;
        self.edges[index] = None;
        Ok(())
    }
    /// Atomically changes an edge's output source.
    ///
    /// The new source is validated with the same direction, type, duplicate,
    /// and cardinality rules as [`connect`](Self::connect); on error the old
    /// edge remains unchanged.
    pub fn reconnect(
        &mut self,
        edge: EdgeId,
        output: impl Into<OutputSlotSelector>,
    ) -> Result<EdgeId, EditError> {
        let edge_index = self.edge_index(edge)?;
        let old = self.edges[edge_index].as_ref().unwrap().clone();
        let output = self.resolve_output(output.into())?;
        let target = self.resolve_input(
            old.target_node.input(
                self.node(old.target_node)?
                    .schema
                    .schema()
                    .inputs
                    .iter()
                    .find(|input| input.id == old.input)
                    .expect("live edge input exists")
                    .name
                    .clone(),
            ),
        )?;
        self.validate_connection(&output, &target, Some(edge))?;
        let record = self.edges[edge_index].as_mut().unwrap();
        record.source_node = output.node;
        record.output = output.slot;
        Ok(edge)
    }
    /// Plans and atomically applies deterministic convenience connections
    /// between two nodes.
    ///
    /// Explicit [`connect`](Self::connect) remains authoritative. The report
    /// describes created edges and inputs with no automatic match.
    pub fn connect_nodes(
        &mut self,
        source: NodeId,
        target: NodeId,
    ) -> Result<AutoConnectReport, EditError> {
        let source_record = self.node(source)?;
        let target_record = self.node(target)?;
        let outputs = source_record.schema.schema().outputs.clone();
        let inputs = target_record.schema.schema().inputs.clone();
        let target_generation = target_record.schema_generation;
        let mut planned = Vec::<(SlotId, SlotId)>::new();
        let mut unmatched_required = Vec::new();
        let mut unmatched_optional = Vec::new();

        for input in &inputs {
            let existing_count = self
                .edges
                .iter()
                .flatten()
                .filter(|edge| edge.target_node == target && edge.input == input.id)
                .count();
            let exposed = self.is_exposed(target, input.id);
            match input.cardinality {
                Cardinality::One if existing_count == 0 && !exposed => {
                    let exact: Vec<_> = outputs
                        .iter()
                        .filter(|output| {
                            output.value_type == input.value_type && output.name == input.name
                        })
                        .collect();
                    let matches = if exact.is_empty() {
                        outputs
                            .iter()
                            .filter(|output| output.value_type == input.value_type)
                            .collect::<Vec<_>>()
                    } else {
                        exact
                    };
                    match matches.as_slice() {
                        [output] => planned.push((output.id, input.id)),
                        [] => self.push_unmatched(
                            target,
                            input,
                            &mut unmatched_required,
                            &mut unmatched_optional,
                        ),
                        _ => {
                            return Err(self.edit_error(
                                EditErrorKind::AmbiguousAutoMatch,
                                Some(target),
                                Some(input.name.clone()),
                            ))
                        }
                    }
                }
                Cardinality::One => {}
                Cardinality::Many if input.auto_collect => {
                    if exposed {
                        return Err(self.edit_error(
                            EditErrorKind::InputSourceConflict,
                            Some(target),
                            Some(input.name.clone()),
                        ));
                    }
                    for output in outputs
                        .iter()
                        .filter(|output| output.value_type == input.value_type)
                    {
                        if !self.has_connection(source, output.id, target, input.id, None)
                            && !planned.contains(&(output.id, input.id))
                        {
                            planned.push((output.id, input.id));
                        }
                    }
                    if existing_count + planned.iter().filter(|(_, id)| *id == input.id).count()
                        == 0
                    {
                        self.push_unmatched(
                            target,
                            input,
                            &mut unmatched_required,
                            &mut unmatched_optional,
                        );
                    }
                }
                Cardinality::Many => {
                    if existing_count == 0 && !exposed {
                        self.push_unmatched(
                            target,
                            input,
                            &mut unmatched_required,
                            &mut unmatched_optional,
                        );
                    }
                }
            }
        }

        let mut edges = Vec::with_capacity(planned.len());
        for (output, input) in planned {
            let output = ResolvedOutput {
                node: source,
                slot: output,
                spec: outputs
                    .iter()
                    .find(|spec| spec.id == output)
                    .unwrap()
                    .clone(),
            };
            let input = ResolvedInput {
                node: target,
                slot: input,
                generation: target_generation,
                spec: inputs.iter().find(|spec| spec.id == input).unwrap().clone(),
            };
            self.validate_connection(&output, &input, None)?;
            edges.push(self.push_edge(output.node, output.slot, input.node, input.slot));
        }
        Ok(AutoConnectReport {
            edges,
            unmatched_required,
            unmatched_optional,
        })
    }

    /// Atomically connects matching outputs from explicit sources to one Many input.
    ///
    /// This is a graph-edit convenience, not a new execution mechanism. It
    /// resolves only the selected target input, then scans each supplied node's
    /// output schema for exact type matches. It never scans other nodes, touches
    /// other inputs, flattens Vec values, or requires/enables auto_collect.
    ///
    /// Existing edges keep their order. New edges follow source iterator order,
    /// then source output declaration order. Repeated sources and existing
    /// output/input pairs are skipped. Distinct output slots remain distinct
    /// even if their values share underlying ownership. Only new EdgeIds return.
    ///
    /// All sources and the target are validated before any edits are applied.
    /// One inputs fail with ExpectedManyInput; exposed targets fail with
    /// InputSourceConflict even for an empty source list. No matches succeed
    /// with an empty result; missing Required input and cycles are compile errors.
    /// Both typed input handles and delayed name selectors are accepted.
    /// Matching is validated transactionally, like ordinary connection edits.
    pub fn collect_into<I: IntoIterator<Item = NodeId>>(
        &mut self,
        sources: I,
        input: impl Into<InputSlotSelector>,
    ) -> Result<Vec<EdgeId>, EditError> {
        let input = self.resolve_input(input.into())?;
        if input.spec.cardinality != Cardinality::Many {
            return Err(self.edit_error(
                EditErrorKind::ExpectedManyInput,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        if self.is_exposed(input.node, input.slot) {
            return Err(self.edit_error(
                EditErrorKind::InputSourceConflict,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        let sources: Vec<NodeId> = sources.into_iter().collect();
        for source in &sources {
            self.node(*source)?;
        }
        let mut planned = Vec::<(NodeId, SlotId)>::new();
        for source in sources {
            for output in &self.node(source)?.schema.schema().outputs {
                if output.value_type == input.spec.value_type
                    && !self.has_connection(source, output.id, input.node, input.slot, None)
                    && !planned.contains(&(source, output.id))
                {
                    planned.push((source, output.id));
                }
            }
        }
        Ok(planned
            .into_iter()
            .map(|(source, output)| self.push_edge(source, output, input.node, input.slot))
            .collect())
    }
    /// Exposes an otherwise unproduced input as a per-run external entry.
    ///
    /// An exposed input cannot also have a normal producer. Compiled versions
    /// freeze their own accepted input definitions.
    pub fn expose_input<T: ValueFor<M>>(
        &mut self,
        input: impl Into<InputSlotSelector>,
    ) -> Result<RunInput<T, M>, EditError> {
        let input = self.resolve_input(input.into())?;
        if input.spec.value_type != SlotTypeId::of::<T>() {
            return Err(self.edit_error(
                EditErrorKind::TypeMismatch,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        if self.incoming_count(input.node, input.slot, None) != 0
            || self.is_exposed(input.node, input.slot)
        {
            return Err(self.edit_error(
                EditErrorKind::InputSourceConflict,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        let binding = self.next_binding;
        self.next_binding = self.next_binding.wrapping_add(1);
        self.exposed.push(ExposedInput {
            node: input.node,
            input: input.slot,
            binding,
            value_type: input.spec.value_type,
            presence: input.spec.presence,
            cardinality: input.spec.cardinality,
        });
        Ok(RunInput::new(
            InputSlot::new(input.node, input.slot, input.generation),
            binding,
        ))
    }
    /// Adds or removes a compilation target.
    ///
    /// Active nodes are roots, not enable flags: compilation includes all of
    /// their upstream dependencies.
    pub fn set_active(
        &mut self,
        node: impl Into<NodeSelector>,
        active: bool,
    ) -> Result<(), EditError> {
        let node = self.resolve_node_selector(node.into())?;
        let index = self.node_index(node)?;
        self.nodes[index].as_mut().unwrap().active = active;
        Ok(())
    }
    /// Fully compiles the current active reverse closure into an immutable
    /// execution version.
    ///
    /// Compilation does not publish a current version and does not mutate this
    /// declaration graph. Global validation is limited to the selected closure.
    pub fn compile(&self) -> Result<ExecutionGraphVersion<M>, CompileError> {
        crate::compiled::compile_graph(self)
    }
}
impl Graph<Local> {
    /// Creates an empty graph that accepts local values and `!Send` futures.
    pub fn new() -> Self {
        Self::new_inner()
    }
    /// Adds a repeatable synchronous local task with its schema.
    /// Accepts ordinary declarations or a pre-bound task layout with keyed I/O.
    pub fn add_sync<F>(
        &mut self,
        name: impl Into<String>,
        schema: impl Into<BoundSchema>,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> LocalTaskResult + 'static,
    {
        self.add_node(name.into(), schema.into(), Task::<Local>::sync(task))
    }
    /// Adds a repeatable asynchronous local task with its schema.
    pub fn add_async<F, Fut>(
        &mut self,
        name: impl Into<String>,
        schema: impl Into<BoundSchema>,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> Fut + 'static,
        Fut: Future<Output = LocalTaskResult> + 'static,
    {
        self.add_node(
            name.into(),
            schema.into(),
            Task::<Local>::asynchronous(task),
        )
    }

    /// Replaces a local synchronous factory without changing schema or edges.
    ///
    /// Equivalent to replace_task with Task::sync. The bound layout and old
    /// compiled versions remain unchanged.
    pub fn replace_sync<F>(&mut self, node: NodeId, task: F) -> Result<(), EditError>
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> LocalTaskResult + 'static,
    {
        self.replace_task(node, Task::<Local>::sync(task))
    }

    /// Replaces a local asynchronous factory; each run receives a fresh Future.
    /// Preserves schema, layout, edges, and old versions.
    pub fn replace_async<F, Fut>(&mut self, node: NodeId, task: F) -> Result<(), EditError>
    where
        F: Fn(TaskContext<Local>, NodeInputs<Local>) -> Fut + 'static,
        Fut: Future<Output = LocalTaskResult> + 'static,
    {
        self.replace_task(node, Task::<Local>::asynchronous(task))
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
        schema: impl Into<BoundSchema>,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> SendTaskResult
            + Send
            + Sync
            + 'static,
    {
        self.add_node(name.into(), schema.into(), Task::<SendMode>::sync(task))
    }
    /// Adds a repeatable asynchronous task whose factory and future are
    /// cross-thread safe.
    pub fn add_async<F, Fut>(
        &mut self,
        name: impl Into<String>,
        schema: impl Into<BoundSchema>,
        task: F,
    ) -> Result<NodeId, EditError>
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = SendTaskResult> + Send + 'static,
    {
        self.add_node(
            name.into(),
            schema.into(),
            Task::<SendMode>::asynchronous(task),
        )
    }

    /// Replaces a Send + Sync synchronous factory without changing its layout.
    /// Preserves schema, edges, and old compiled versions.
    pub fn replace_sync<F>(&mut self, node: NodeId, task: F) -> Result<(), EditError>
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> SendTaskResult
            + Send
            + Sync
            + 'static,
    {
        self.replace_task(node, Task::<SendMode>::sync(task))
    }

    /// Replaces a Send + Sync asynchronous factory producing fresh Send Futures.
    /// Preserves schema, layout, edges, and old versions.
    pub fn replace_async<F, Fut>(&mut self, node: NodeId, task: F) -> Result<(), EditError>
    where
        F: Fn(TaskContext<SendMode>, NodeInputs<SendMode>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = SendTaskResult> + Send + 'static,
    {
        self.replace_task(node, Task::<SendMode>::asynchronous(task))
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
            id: GraphId::new(NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed)),
            nodes: Vec::new(),
            edges: Vec::new(),
            names: HashMap::new(),
            exposed: Vec::new(),
            next_node: 1,
            next_edge: 1,
            next_binding: 1,
            _mode: PhantomData,
        }
    }

    fn add_node(
        &mut self,
        name: String,
        schema: BoundSchema,
        task: Task<M>,
    ) -> Result<NodeId, EditError> {
        self.validate_node_name(&name, None)?;
        schema.schema().validate()?;
        let id = NodeId::new(self.id, self.next_node);
        self.next_node = self.next_node.wrapping_add(1);
        self.names.insert(name.clone(), id);
        self.nodes.push(Some(NodeRecord {
            id,
            name,
            schema,
            schema_generation: 1,
            task,
            active: false,
        }));
        Ok(id)
    }

    fn validate_node_name(&self, name: &str, current: Option<NodeId>) -> Result<(), EditError> {
        if name.is_empty() {
            return Err(self.edit_error(
                EditErrorKind::InvalidNodeName,
                current,
                Some(name.into()),
            ));
        }
        if self
            .names
            .get(name)
            .is_some_and(|found| Some(*found) != current)
        {
            return Err(self.edit_error(
                EditErrorKind::DuplicateNodeName,
                current,
                Some(name.into()),
            ));
        }
        Ok(())
    }

    pub(crate) fn node(&self, node: NodeId) -> Result<&NodeRecord<M>, EditError> {
        let index = self.node_index(node)?;
        Ok(self.nodes[index].as_ref().unwrap())
    }

    fn node_index(&self, node: NodeId) -> Result<usize, EditError> {
        if node.graph() != self.id {
            return Err(self.edit_error(EditErrorKind::ForeignHandle, Some(node), None));
        }
        let index = node
            .raw()
            .checked_sub(1)
            .and_then(|raw| usize::try_from(raw).ok());
        match index.filter(|index| self.nodes.get(*index).is_some_and(Option::is_some)) {
            Some(index) => Ok(index),
            None => Err(self.edit_error(EditErrorKind::StaleNodeId, Some(node), None)),
        }
    }

    fn edge_index(&self, edge: EdgeId) -> Result<usize, EditError> {
        if edge.graph() != self.id {
            return Err(self.edit_error(EditErrorKind::ForeignHandle, None, None));
        }
        let index = edge
            .raw()
            .checked_sub(1)
            .and_then(|raw| usize::try_from(raw).ok());
        match index.filter(|index| self.edges.get(*index).is_some_and(Option::is_some)) {
            Some(index) => Ok(index),
            None => Err(EditError::new(
                EditErrorKind::StaleEdgeId,
                ErrorContext {
                    graph: Some(self.id),
                    edge: Some(edge),
                    ..ErrorContext::default()
                },
            )),
        }
    }

    fn resolve_node_selector(&self, selector: NodeSelector) -> Result<NodeId, EditError> {
        match selector.into_parts() {
            Ok(node) => {
                self.node_index(node)?;
                Ok(node)
            }
            Err(name) => {
                self.names.get(&name).copied().ok_or_else(|| {
                    self.edit_error(EditErrorKind::UnknownNodeName, None, Some(name))
                })
            }
        }
    }

    fn resolve_input(&self, selector: InputSlotSelector) -> Result<ResolvedInput, EditError> {
        let (node, name, slot, generation) = selector.into_parts();
        let record = self.node(node)?;
        if generation.is_some_and(|generation| generation != record.schema_generation) {
            return Err(self.edit_error(EditErrorKind::StaleSlotHandle, Some(node), None));
        }
        let spec = if let Some(slot) = slot {
            record
                .schema
                .schema()
                .inputs
                .iter()
                .find(|input| input.id == slot)
        } else {
            record
                .schema
                .schema()
                .inputs
                .iter()
                .find(|input| input.name == name)
        };
        let spec = spec.cloned().ok_or_else(|| {
            self.edit_error(
                if slot.is_some() {
                    EditErrorKind::StaleSlotHandle
                } else {
                    EditErrorKind::UnknownSlotName
                },
                Some(node),
                (!name.is_empty()).then_some(name),
            )
        })?;
        Ok(ResolvedInput {
            node,
            slot: spec.id,
            generation: record.schema_generation,
            spec,
        })
    }

    fn resolve_output(&self, selector: OutputSlotSelector) -> Result<ResolvedOutput, EditError> {
        let (node, name, slot, generation) = selector.into_parts();
        let record = self.node(node)?;
        if generation.is_some_and(|generation| generation != record.schema_generation) {
            return Err(self.edit_error(EditErrorKind::StaleSlotHandle, Some(node), None));
        }
        let spec = if let Some(slot) = slot {
            record
                .schema
                .schema()
                .outputs
                .iter()
                .find(|output| output.id == slot)
        } else {
            record
                .schema
                .schema()
                .outputs
                .iter()
                .find(|output| output.name == name)
        };
        let spec = spec.cloned().ok_or_else(|| {
            self.edit_error(
                if slot.is_some() {
                    EditErrorKind::StaleSlotHandle
                } else {
                    EditErrorKind::UnknownSlotName
                },
                Some(node),
                (!name.is_empty()).then_some(name),
            )
        })?;
        Ok(ResolvedOutput {
            node,
            slot: spec.id,
            spec,
        })
    }

    fn validate_connection(
        &self,
        output: &ResolvedOutput,
        input: &ResolvedInput,
        excluding: Option<EdgeId>,
    ) -> Result<(), EditError> {
        if output.spec.value_type != input.spec.value_type {
            return Err(self.edit_error(
                EditErrorKind::TypeMismatch,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        if self.is_exposed(input.node, input.slot) {
            return Err(self.edit_error(
                EditErrorKind::InputSourceConflict,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        if self.has_connection(output.node, output.slot, input.node, input.slot, excluding) {
            return Err(self.edit_error(
                EditErrorKind::DuplicateEdge,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        if input.spec.cardinality == Cardinality::One
            && self.incoming_count(input.node, input.slot, excluding) != 0
        {
            return Err(self.edit_error(
                EditErrorKind::CardinalityOverflow,
                Some(input.node),
                Some(input.spec.name.clone()),
            ));
        }
        Ok(())
    }

    fn has_connection(
        &self,
        source_node: NodeId,
        output: SlotId,
        target_node: NodeId,
        input: SlotId,
        excluding: Option<EdgeId>,
    ) -> bool {
        self.edges.iter().flatten().any(|edge| {
            Some(edge.id) != excluding
                && edge.source_node == source_node
                && edge.output == output
                && edge.target_node == target_node
                && edge.input == input
        })
    }

    fn incoming_count(&self, node: NodeId, input: SlotId, excluding: Option<EdgeId>) -> usize {
        self.edges
            .iter()
            .flatten()
            .filter(|edge| {
                Some(edge.id) != excluding && edge.target_node == node && edge.input == input
            })
            .count()
    }

    fn is_exposed(&self, node: NodeId, input: SlotId) -> bool {
        self.exposed
            .iter()
            .any(|binding| binding.node == node && binding.input == input)
    }

    fn push_edge(
        &mut self,
        source_node: NodeId,
        output: SlotId,
        target_node: NodeId,
        input: SlotId,
    ) -> EdgeId {
        let id = EdgeId::new(self.id, self.next_edge);
        self.next_edge = self.next_edge.wrapping_add(1);
        self.edges.push(Some(EdgeRecord {
            id,
            source_node,
            output,
            target_node,
            input,
        }));
        id
    }

    fn push_unmatched(
        &self,
        node: NodeId,
        input: &InputSpec,
        required: &mut Vec<InputSlotSelector>,
        optional: &mut Vec<InputSlotSelector>,
    ) {
        match input.presence {
            Presence::Required => required.push(node.input(input.name.clone())),
            Presence::Optional => optional.push(node.input(input.name.clone())),
        }
    }

    fn slot_error(&self, kind: EditErrorKind, node: NodeId, name: &str) -> EditError {
        self.edit_error(kind, Some(node), Some(name.to_owned()))
    }

    fn edit_error(
        &self,
        kind: EditErrorKind,
        node: Option<NodeId>,
        name: Option<String>,
    ) -> EditError {
        EditError::new(
            kind,
            ErrorContext {
                graph: Some(self.id),
                node,
                name,
                ..ErrorContext::default()
            },
        )
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
